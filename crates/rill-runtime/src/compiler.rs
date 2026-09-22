//! Compile expressions once, preserving staged unary calls and effect order.
use crate::code::{Capture, Code, FunctionCode, Instruction, Reference, Source};
use rill_syntax::{
    ast::{Arm, Binary, ExprId, ExprKind, Literal, Module, Pattern, Rest, Statement, Unary},
    token::Span,
};
use std::{collections::BTreeSet, rc::Rc};

struct Compiler<'a> {
    module: &'a Module,
    code: Code,
    bound: Vec<(usize, BTreeSet<String>)>,
    capture: bool,
    free: BTreeSet<String>,
    constants: indexmap::IndexSet<Literal>,
}

pub fn compile(module: &Module, context: Option<usize>) -> Rc<Code> {
    let source = Rc::new(Source {
        name: module.name.clone(),
        text: module.source.clone(),
    });
    let mut compiler = Compiler::new(module, source);
    compiler.code.context = context;
    if module.statements.is_empty() {
        compiler.constant(Literal::Unit, 0..0);
    }
    for (index, statement) in module.statements.iter().enumerate() {
        compiler.statement(statement, false);
        compiler.emit(
            Instruction::Boundary {
                last: index + 1 == module.statements.len(),
            },
            0..module.source.len(),
        );
        if index + 1 < module.statements.len() {
            compiler.emit(Instruction::Pop, 0..0);
        }
    }
    compiler.emit(Instruction::Return, 0..module.source.len());
    compiler.finish()
}

impl<'a> Compiler<'a> {
    fn new(module: &'a Module, source: Rc<Source>) -> Self {
        Self {
            module,
            code: Code {
                source,
                instructions: Vec::new(),
                constants: Vec::new(),
                layouts: vec![Rc::new(indexmap::IndexSet::new())],
                context: None,
            },
            bound: vec![(0, BTreeSet::new())],
            capture: false,
            free: BTreeSet::new(),
            constants: indexmap::IndexSet::new(),
        }
    }
    fn finish(mut self) -> Rc<Code> {
        self.code.constants = self.constants.into_iter().collect();
        Rc::new(self.code)
    }
    fn constant(&mut self, literal: Literal, span: Span) {
        let (index, _) = self.constants.insert_full(literal);
        self.emit(Instruction::Constant(index), span);
    }
    fn emit(&mut self, instruction: Instruction, span: Span) -> usize {
        let offset = self.code.instructions.len();
        self.code.instructions.push((instruction, span));
        offset
    }
    fn reference(&mut self, name: &str) -> Reference {
        for (scope, (layout, names)) in self.bound.iter().enumerate().rev() {
            if names.contains(name) {
                let slot = self.code.layouts[*layout]
                    .get_index_of(name)
                    .expect("bound slot");
                return Reference::Local { scope, slot };
            }
        }
        if self.capture {
            self.free.insert(name.into());
            let (slot, _) = Rc::make_mut(&mut self.code.layouts[0]).insert_full(name.into());
            Reference::Local { scope: 0, slot }
        } else {
            Reference::Global(name.into())
        }
    }
    fn binding(&mut self, pattern: &Pattern) {
        let mut names = Vec::new();
        bindings(pattern, &mut names);
        let (layout, bound) = self.bound.last_mut().expect("compiler lexical scope");
        Rc::make_mut(&mut self.code.layouts[*layout]).extend(names.iter().cloned());
        bound.extend(names);
    }
    fn enter(&mut self, span: Span) {
        let layout = self.code.layouts.len();
        self.code.layouts.push(Rc::new(indexmap::IndexSet::new()));
        self.bound.push((layout, BTreeSet::new()));
        self.emit(Instruction::Enter(layout), span);
    }
    fn block(&mut self, statements: &[Statement], tail: bool) {
        if statements.is_empty() {
            self.constant(Literal::Unit, 0..0);
        }
        for (index, statement) in statements.iter().enumerate() {
            self.statement(statement, tail && index + 1 == statements.len());
            if index + 1 < statements.len() {
                self.emit(Instruction::Pop, 0..0);
            }
        }
    }
    fn statement(&mut self, statement: &Statement, tail: bool) {
        match statement {
            Statement::Expression(expr) => self.expression(*expr, tail),
            Statement::Let(pattern, expr) => {
                self.expression(*expr, false);
                self.pattern_references(pattern);
                self.binding(pattern);
                self.emit(
                    Instruction::Bind(pattern.clone()),
                    self.module.expression(*expr).span.clone(),
                );
                self.constant(Literal::Unit, 0..0);
            }
            Statement::Functions(functions) => {
                for f in functions {
                    self.binding(&Pattern::Bind(f.name.clone()));
                }
                let codes = functions
                    .iter()
                    .map(|f| {
                        self.function(
                            Some(f.name.clone()),
                            &f.parameters,
                            f.body,
                            Some(f.span.start),
                        )
                    })
                    .collect();
                self.emit(Instruction::Functions(codes), 0..0);
                self.constant(Literal::Unit, 0..0);
            }
            Statement::Struct(name, fields) => {
                self.binding(&Pattern::Bind(name.clone()));
                self.emit(Instruction::Struct(name.clone(), fields.clone()), 0..0);
                self.constant(Literal::Unit, 0..0);
            }
            Statement::Enum(name, cases) => {
                self.binding(&Pattern::Bind(name.clone()));
                self.emit(Instruction::Enum(name.clone(), cases.clone()), 0..0);
                self.constant(Literal::Unit, 0..0);
            }
            Statement::Import { path, name } => {
                self.binding(&Pattern::Bind(name.clone()));
                self.emit(
                    Instruction::Import {
                        path: path.clone(),
                        name: name.clone(),
                    },
                    0..self.module.source.len(),
                );
                self.constant(Literal::Unit, 0..0);
            }
            Statement::Export(fields) => {
                self.emit(
                    Instruction::Export(fields.clone()),
                    0..self.module.source.len(),
                );
                self.constant(Literal::Unit, 0..0);
            }
            Statement::Command(stages) => {
                self.plan(stages);
                self.emit(Instruction::RunPlan, 0..self.module.source.len());
            }
        }
    }
    fn function(
        &mut self,
        name: Option<String>,
        parameters: &[Pattern],
        body: ExprId,
        declaration: Option<usize>,
    ) -> Rc<FunctionCode> {
        let mut compiler = Self::new(self.module, Rc::clone(&self.code.source));
        compiler.capture = true;
        if let Some(name) = &name {
            compiler.binding(&Pattern::Bind(name.clone()));
        }
        for pattern in parameters {
            compiler.pattern_references(pattern);
            compiler.binding(pattern);
        }
        compiler.expression(body, true);
        compiler.emit(
            Instruction::Return,
            self.module.expression(body).span.clone(),
        );
        let captures = compiler
            .free
            .iter()
            .map(|name| Capture {
                name: name.clone(),
                slot: compiler.code.layouts[0]
                    .get_index_of(name)
                    .expect("capture slot"),
                source: self.reference(name),
            })
            .collect();
        let documentation = declaration.and_then(|start| {
            let mut lines: Vec<_> = self.module.source[..start]
                .trim_end_matches([' ', '\t', '\r'])
                .lines()
                .rev()
                .map_while(|line| line.trim_start().strip_prefix("##").map(str::trim))
                .collect();
            lines.reverse();
            (!lines.is_empty()).then(|| lines.join("\n"))
        });
        let function = FunctionCode {
            documentation,
            name,
            parameters: parameters.to_vec(),
            captures,
            body: compiler.finish(),
        };
        Rc::new(function)
    }
    fn pattern_references(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Constructor(path, payload) => {
                self.reference(&path[0]);
                if let Some(payload) = payload {
                    self.pattern_references(payload);
                }
            }
            Pattern::List(items, _) => {
                for item in items {
                    self.pattern_references(item);
                }
            }
            Pattern::Record(fields, _) => {
                for (_, p) in fields {
                    self.pattern_references(p);
                }
            }
            _ => {}
        }
    }
    fn expression(&mut self, id: ExprId, tail: bool) {
        let expression = self.module.expression(id);
        let span = expression.span.clone();
        match &expression.kind {
            ExprKind::Literal(literal) => {
                self.constant(literal.clone(), span);
            }
            ExprKind::Group(id) => self.expression(*id, tail),
            ExprKind::Name(name) => {
                let reference = self.reference(name);
                self.emit(Instruction::Load(reference), span);
            }
            ExprKind::List(items) => {
                for item in items {
                    self.expression(*item, false);
                }
                self.emit(Instruction::List(items.len()), span);
            }
            ExprKind::Record(fields) => {
                for (_, value) in fields {
                    self.expression(*value, false);
                }
                self.emit(
                    Instruction::Record(fields.iter().map(|(key, _)| key.clone()).collect()),
                    span,
                );
            }
            ExprKind::Call(function, argument) => {
                self.expression(*function, false);
                self.expression(*argument, false);
                self.emit(Instruction::Call { tail }, span);
            }
            ExprKind::Field(base, field) => {
                self.expression(*base, false);
                self.emit(Instruction::Field(field.clone()), span);
            }
            ExprKind::Index(base, index) => {
                self.expression(*base, false);
                self.expression(*index, false);
                self.emit(Instruction::Index, span);
            }
            ExprKind::Unary(op, child) => {
                if *op == Unary::Negate
                    && let ExprKind::Literal(Literal::Integer(text)) =
                        &self.module.expression(*child).kind
                {
                    self.constant(Literal::Integer(format!("-{text}")), span);
                    return;
                }
                self.expression(*child, false);
                self.emit(Instruction::Unary(*op), span);
            }
            ExprKind::Binary(op, left, right) => self.binary(*op, *left, *right, span, tail),
            ExprKind::If { condition, yes, no } => {
                self.expression(*condition, false);
                let branch = self.emit(Instruction::Branch(0), span.clone());
                self.expression(*yes, tail);
                let jump = self.emit(Instruction::Jump(0), span);
                self.code.instructions[branch].0 =
                    Instruction::Branch(self.code.instructions.len());
                self.expression(*no, tail);
                self.code.instructions[jump].0 = Instruction::Jump(self.code.instructions.len());
            }
            ExprKind::Block(statements) => {
                self.enter(span.clone());
                self.block(statements, tail);
                self.bound.pop();
                self.emit(Instruction::Leave, span);
            }
            ExprKind::Closure {
                name,
                parameters,
                body,
            } => {
                let code = self.function(name.clone(), parameters, *body, None);
                self.emit(Instruction::Closure(code), span);
            }
            ExprKind::Match { subject, arms } => self.matching(*subject, arms, tail, span),
            ExprKind::Plan(stages) => self.plan(stages),
        }
    }
    fn plan(&mut self, stages: &[rill_syntax::ast::Stage]) {
        use rill_syntax::ast::{Argument, CommandPart};
        let span = 0..self.module.source.len();
        self.emit(Instruction::BeginPlan, span.clone());
        for stage in stages {
            self.emit(Instruction::BeginStage, span.clone());
            for part in &stage.parts {
                let argument = match part {
                    CommandPart::Argument(argument) => Some(argument),
                    CommandPart::Redirect(_, argument) => argument.as_ref(),
                };
                if let Some(argument) = argument {
                    match argument {
                        Argument::Literal(text) => {
                            self.constant(Literal::String(text.clone()), span.clone());
                        }
                        Argument::Value(value) | Argument::Spread(value) => {
                            self.expression(*value, false);
                        }
                    }
                }
                let instruction = match part {
                    CommandPart::Argument(argument) => Instruction::Argument {
                        spread: matches!(argument, Argument::Spread(_)),
                    },
                    CommandPart::Redirect(kind, _) => Instruction::Redirect(*kind),
                };
                self.emit(instruction, span.clone());
            }
        }
        self.emit(Instruction::EndPlan, span);
    }
    fn matching(&mut self, subject: ExprId, arms: &[Arm], tail: bool, span: Span) {
        self.expression(subject, false);
        let mut ends = Vec::new();
        for arm in arms {
            self.enter(span.clone());
            self.pattern_references(&arm.pattern);
            self.binding(&arm.pattern);
            let attempt = self.emit(Instruction::TryBind(arm.pattern.clone(), 0), span.clone());
            let guard = arm.guard.map(|guard| {
                self.expression(guard, false);
                self.emit(Instruction::Branch(0), span.clone())
            });
            self.emit(Instruction::Pop, span.clone());
            self.expression(arm.body, tail);
            self.emit(Instruction::Leave, span.clone());
            ends.push(self.emit(Instruction::Jump(0), span.clone()));
            let failure = self.code.instructions.len();
            self.code.instructions[attempt].0 = Instruction::TryBind(arm.pattern.clone(), failure);
            if let Some(guard) = guard {
                self.code.instructions[guard].0 = Instruction::Branch(failure);
            }
            self.emit(Instruction::Leave, span.clone());
            self.bound.pop();
        }
        self.emit(Instruction::NoMatch, span);
        for end in ends {
            self.code.instructions[end].0 = Instruction::Jump(self.code.instructions.len());
        }
    }
    fn binary(&mut self, op: Binary, left: ExprId, right: ExprId, span: Span, tail: bool) {
        self.expression(left, false);
        if matches!(op, Binary::And | Binary::Or) {
            let branch = self.emit(Instruction::Branch(0), span.clone());
            if op == Binary::Or {
                self.constant(Literal::Bool(true), span.clone());
            } else {
                self.expression(right, false);
                self.emit(Instruction::Bool, span.clone());
            }
            let jump = self.emit(Instruction::Jump(0), span.clone());
            self.code.instructions[branch].0 = Instruction::Branch(self.code.instructions.len());
            if op == Binary::And {
                self.constant(Literal::Bool(false), span);
            } else {
                self.expression(right, false);
                self.emit(Instruction::Bool, span);
            }
            self.code.instructions[jump].0 = Instruction::Jump(self.code.instructions.len());
        } else {
            self.expression(right, false);
            if op == Binary::Pipe {
                self.emit(Instruction::Swap, span.clone());
                self.emit(Instruction::Call { tail }, span);
            } else {
                self.emit(Instruction::Binary(op), span);
            }
        }
    }
}

fn bindings(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Bind(name) => names.push(name.clone()),
        Pattern::List(items, rest) => {
            for item in items {
                bindings(item, names);
            }
            if let Some(name) = rest
                && name != "_"
            {
                names.push(name.clone());
            }
        }
        Pattern::Record(fields, rest) => {
            for (_, pattern) in fields {
                bindings(pattern, names);
            }
            if let Rest::Bind(name) = rest
                && name != "_"
            {
                names.push(name.clone());
            }
        }
        Pattern::Constructor(_, Some(pattern)) => bindings(pattern, names),
        _ => {}
    }
}
