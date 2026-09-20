//! Contiguous expression storage; indices remain stable when its vectors grow.

use crate::token::{Redirect, Span};

/// Expression index scoped to one [`Module`]; never mix indices from different parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExprId(pub(crate) usize);

/// Owned source and expressions; no node borrows a parser buffer.
#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub source: String,
    pub statements: Vec<Statement>,
    pub(crate) nodes: Vec<Expr>,
}

impl Module {
    /// Look up an expression produced by this module's parser.
    ///
    /// # Panics
    /// May panic if `id` belongs to another module. Indices carry no cross-module identity.
    #[must_use]
    pub fn expression(&self, id: ExprId) -> &Expr {
        &self.nodes[id.0]
    }
}

/// Expression and its complete original source range.
#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Literal(Literal),
    Group(ExprId),
    Name(String),
    List(Vec<ExprId>),
    Record(Vec<(String, ExprId)>),
    Call(ExprId, ExprId),
    Field(ExprId, String),
    Index(ExprId, ExprId),
    Unary(Unary, ExprId),
    Binary(Binary, ExprId, ExprId),
    If {
        condition: ExprId,
        yes: ExprId,
        no: ExprId,
    },
    Block(Vec<Statement>),
    Closure {
        name: Option<String>,
        parameters: Vec<Pattern>,
        body: ExprId,
    },
    Match {
        subject: ExprId,
        arms: Vec<Arm>,
    },
    Plan(Vec<Stage>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Literal {
    Unit,
    Null,
    Bool(bool),
    Integer(String),
    Float(String),
    String(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Negate,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binary {
    Multiply,
    Divide,
    Add,
    Subtract,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    With,
    Pipe,
}

/// A declaration does not contribute a block's result value.
#[derive(Clone, Debug)]
pub enum Statement {
    Expression(ExprId),
    Let(Pattern, ExprId),
    Functions(Vec<Function>),
    Struct(String, Vec<String>),
    Enum(String, Vec<(String, Vec<String>)>),
    Import { path: String, name: String },
    Export(Vec<(String, Vec<String>)>),
    Command(Vec<Stage>),
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub parameters: Vec<Pattern>,
    pub body: ExprId,
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<ExprId>,
    pub body: ExprId,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Ignore,
    Bind(String),
    Literal(Literal),
    List(Vec<Self>, Option<String>),
    Record(Vec<(String, Self)>, Rest),
    Constructor(Vec<String>, Option<Box<Self>>),
}

/// Closed shape, ignored surplus fields, or a binding for the surplus record.
#[derive(Clone, Debug)]
pub enum Rest {
    Exact,
    Ignore,
    Bind(String),
}

/// Arguments and redirections retain their relative evaluation order.
#[derive(Clone, Debug)]
pub struct Stage {
    pub parts: Vec<CommandPart>,
}

#[derive(Clone, Debug)]
pub enum CommandPart {
    Argument(Argument),
    Redirect(Redirect, Option<Argument>),
}

#[derive(Clone, Debug)]
pub enum Argument {
    Literal(String),
    Value(ExprId),
    Spread(ExprId),
}

impl ExprKind {
    pub(crate) fn children(&self, visit: &mut impl FnMut(ExprId)) {
        match self {
            Self::Group(id) | Self::Field(id, _) | Self::Unary(_, id) => visit(*id),
            Self::Call(a, b) | Self::Index(a, b) | Self::Binary(_, a, b) => {
                visit(*a);
                visit(*b);
            }
            Self::List(items) => items.iter().copied().for_each(visit),
            Self::Record(fields) => fields.iter().for_each(|(_, id)| visit(*id)),
            Self::If { condition, yes, no } => {
                visit(*condition);
                visit(*yes);
                visit(*no);
            }
            Self::Closure { body, .. } => visit(*body),
            Self::Match { subject, arms } => {
                visit(*subject);
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        visit(guard);
                    }
                    visit(arm.body);
                }
            }
            Self::Block(statements) => statements
                .iter()
                .for_each(|statement| statement.children(visit)),
            Self::Plan(stages) => stages.iter().for_each(|stage| stage.children(visit)),
            Self::Literal(_) | Self::Name(_) => {}
        }
    }
}

impl Statement {
    fn children(&self, visit: &mut impl FnMut(ExprId)) {
        match self {
            Self::Expression(id) | Self::Let(_, id) => visit(*id),
            Self::Functions(functions) => functions.iter().for_each(|f| visit(f.body)),
            Self::Command(stages) => stages.iter().for_each(|s| s.children(visit)),
            Self::Struct(..) | Self::Enum(..) | Self::Import { .. } | Self::Export(_) => {}
        }
    }
}

impl Stage {
    fn children(&self, visit: &mut impl FnMut(ExprId)) {
        for part in &self.parts {
            let argument = match part {
                CommandPart::Argument(arg) | CommandPart::Redirect(_, Some(arg)) => arg,
                CommandPart::Redirect(_, None) => continue,
            };
            if let Argument::Value(id) | Argument::Spread(id) = argument {
                visit(*id);
            }
        }
    }
}
