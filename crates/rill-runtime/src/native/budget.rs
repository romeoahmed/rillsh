//! Known retained payload sizes, with shared objects and code counted once per operation.
//! Private allocator and collection bucket overheads are deliberately not estimated.
use crate::{Error, value::Value};
use gc_arena::{Collect, Gc};
use std::{collections::HashSet, mem::size_of};

#[derive(Collect)]
#[collect(require_static)]
pub struct Budget {
    pub(super) remaining: usize,
    pub(super) seen: HashSet<usize>,
    pub(super) kind: Option<&'static str>,
}
impl Budget {
    pub fn new(remaining: usize) -> Self {
        Self {
            remaining,
            seen: HashSet::new(),
            kind: None,
        }
    }
    pub(super) fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(|| Error::new("LimitExceeded", "retained values exceed the byte limit"))?;
        Ok(())
    }
    pub fn value(&mut self, value: Value<'_>) -> Result<(), Error> {
        self.charge(size_of::<Value<'_>>())?;
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                Value::String(s) if self.seen.insert(Gc::as_ptr(s).addr()) => {
                    self.charge(size_of::<String>().saturating_add(s.capacity()))?;
                }
                Value::Bytes(b) if self.seen.insert(Gc::as_ptr(b).addr()) => {
                    self.charge(size_of::<crate::value::Blob>().saturating_add(b.1))?;
                }
                Value::Path(p) if self.seen.insert(Gc::as_ptr(p).addr()) => {
                    self.charge(
                        size_of::<crate::value::NativePath>().saturating_add(p.0.capacity()),
                    )?;
                }
                Value::List(items) => {
                    let storage = items.storage();
                    if self.seen.insert(Gc::as_ptr(storage).addr()) {
                        self.charge(storage.capacity().saturating_mul(size_of::<Value<'_>>()))?;
                        pending.extend_from_slice(&storage);
                    }
                }
                Value::Record(fields) if self.seen.insert(Gc::as_ptr(fields).addr()) => {
                    self.charge(
                        fields
                            .capacity()
                            .saturating_mul(size_of::<(String, Value<'_>)>()),
                    )?;
                    for (name, value) in fields.iter() {
                        self.charge(name.capacity())?;
                        pending.push(*value);
                    }
                }
                Value::Adt(adt) if self.seen.insert(Gc::as_ptr(adt).addr()) => {
                    self.charge(size_of::<crate::value::Adt<'_>>())?;
                    pending.push(Value::Record(adt.fields));
                    pending.push(Value::Constructor(adt.descriptor));
                }
                Value::Function(closure) if self.seen.insert(Gc::as_ptr(closure).addr()) => {
                    self.charge(size_of::<crate::value::Closure<'_>>())?;
                    self.function(&closure.code)?;
                    if self.seen.insert(Gc::as_ptr(closure.constants).addr()) {
                        let constants = closure.constants.borrow();
                        self.charge(
                            constants
                                .capacity()
                                .saturating_mul(size_of::<Option<Value<'_>>>()),
                        )?;
                        pending.extend(constants.iter().flatten().copied());
                    }
                    if self.seen.insert(Gc::as_ptr(closure.environment).addr()) {
                        let environment = closure.environment.borrow();
                        self.layout(&environment.layout)?;
                        self.charge(
                            environment
                                .slots
                                .capacity()
                                .saturating_mul(size_of::<Option<Value<'_>>>()),
                        )?;
                        pending.extend(environment.values().copied());
                    }
                }
                Value::Constructor(descriptor)
                    if self.seen.insert(Gc::as_ptr(descriptor).addr()) =>
                {
                    self.charge(size_of::<crate::value::Descriptor>())?;
                    self.charge(descriptor.name.capacity())?;
                    self.strings(&descriptor.fields)?;
                }
                Value::Plan(plan) if self.seen.insert(Gc::as_ptr(plan).addr()) => {
                    self.plan(&plan.0)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl Budget {
    fn strings(&mut self, strings: &Vec<String>) -> Result<(), Error> {
        self.charge(strings.capacity().saturating_mul(size_of::<String>()))?;
        for text in strings {
            self.charge(text.capacity())?;
        }
        Ok(())
    }
    fn literal(&mut self, literal: &rill_syntax::ast::Literal) -> Result<(), Error> {
        use rill_syntax::ast::Literal;
        match literal {
            Literal::Integer(text) | Literal::Float(text) | Literal::String(text) => {
                self.charge(text.capacity())
            }
            _ => Ok(()),
        }
    }
    fn pattern(&mut self, pattern: &rill_syntax::ast::Pattern) -> Result<(), Error> {
        use rill_syntax::ast::{Pattern, Rest};
        let mut pending = vec![pattern];
        while let Some(pattern) = pending.pop() {
            match pattern {
                Pattern::Ignore => {}
                Pattern::Bind(name) => self.charge(name.capacity())?,
                Pattern::Literal(literal) => self.literal(literal)?,
                Pattern::List(items, rest) => {
                    self.charge(items.capacity().saturating_mul(size_of::<Pattern>()))?;
                    pending.extend(items);
                    if let Some(name) = rest {
                        self.charge(name.capacity())?;
                    }
                }
                Pattern::Record(fields, rest) => {
                    self.charge(
                        fields
                            .capacity()
                            .saturating_mul(size_of::<(String, Pattern)>()),
                    )?;
                    for (name, child) in fields {
                        self.charge(name.capacity())?;
                        pending.push(child);
                    }
                    if let Rest::Bind(name) = rest {
                        self.charge(name.capacity())?;
                    }
                }
                Pattern::Constructor(path, child) => {
                    self.strings(path)?;
                    if let Some(child) = child {
                        self.charge(size_of::<Pattern>())?;
                        pending.push(child);
                    }
                }
            }
        }
        Ok(())
    }
    fn layout(&mut self, layout: &std::rc::Rc<indexmap::IndexSet<String>>) -> Result<(), Error> {
        if self.seen.insert(std::rc::Rc::as_ptr(layout).addr()) {
            self.charge(layout.capacity().saturating_mul(size_of::<String>()))?;
            for name in layout.iter() {
                self.charge(name.capacity())?;
            }
        }
        Ok(())
    }
    fn function(&mut self, root: &std::rc::Rc<crate::code::FunctionCode>) -> Result<(), Error> {
        use crate::code::{Capture, Code, FunctionCode, Instruction, Reference};
        use rill_syntax::ast::{Literal, Pattern};
        use std::rc::Rc;
        let mut pending = vec![root];
        while let Some(function) = pending.pop() {
            if !self.seen.insert(Rc::as_ptr(function).addr()) {
                continue;
            }
            self.charge(size_of::<FunctionCode>())?;
            if let Some(name) = &function.name {
                self.charge(name.capacity())?;
            }
            self.charge(
                function
                    .parameters
                    .capacity()
                    .saturating_mul(size_of::<Pattern>()),
            )?;
            for pattern in &function.parameters {
                self.pattern(pattern)?;
            }
            self.charge(
                function
                    .captures
                    .capacity()
                    .saturating_mul(size_of::<Capture>()),
            )?;
            for capture in &function.captures {
                self.charge(capture.name.capacity())?;
                if let Reference::Global(name) = &capture.source {
                    self.charge(name.capacity())?;
                }
            }
            let code = &function.body;
            if !self.seen.insert(Rc::as_ptr(code).addr()) {
                continue;
            }
            self.charge(size_of::<Code>())?;
            if self.seen.insert(Rc::as_ptr(&code.source).addr()) {
                self.charge(size_of::<crate::code::Source>())?;
                self.charge(code.source.name.capacity())?;
                self.charge(code.source.text.capacity())?;
            }
            self.charge(
                code.constants
                    .capacity()
                    .saturating_mul(size_of::<Literal>()),
            )?;
            for constant in &code.constants {
                self.literal(constant)?;
            }
            self.charge(
                code.layouts
                    .capacity()
                    .saturating_mul(size_of::<Rc<indexmap::IndexSet<String>>>()),
            )?;
            for layout in &code.layouts {
                self.layout(layout)?;
            }
            self.charge(
                code.instructions
                    .capacity()
                    .saturating_mul(size_of::<(Instruction, rill_syntax::token::Span)>()),
            )?;
            for (instruction, _) in &code.instructions {
                match instruction {
                    Instruction::Closure(function) => pending.push(function),
                    Instruction::Functions(functions) => {
                        self.charge(
                            functions
                                .capacity()
                                .saturating_mul(size_of::<Rc<FunctionCode>>()),
                        )?;
                        pending.extend(functions);
                    }
                    Instruction::Bind(pattern) | Instruction::TryBind(pattern, _) => {
                        self.pattern(pattern)?;
                    }
                    Instruction::Load(Reference::Global(name)) | Instruction::Field(name) => {
                        self.charge(name.capacity())?;
                    }
                    Instruction::Record(names) => self.strings(names)?,
                    Instruction::Struct(name, fields) => {
                        self.charge(name.capacity())?;
                        self.strings(fields)?;
                    }
                    Instruction::Enum(name, variants) => {
                        self.charge(name.capacity())?;
                        self.exports(variants)?;
                    }
                    Instruction::Export(exports) => self.exports(exports)?,
                    Instruction::Import { path, name } => {
                        self.charge(path.capacity())?;
                        self.charge(name.capacity())?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
    fn exports(&mut self, fields: &Vec<(String, Vec<String>)>) -> Result<(), Error> {
        self.charge(
            fields
                .capacity()
                .saturating_mul(size_of::<(String, Vec<String>)>()),
        )?;
        for (name, names) in fields {
            self.charge(name.capacity())?;
            self.strings(names)?;
        }
        Ok(())
    }
    fn plan(&mut self, plan: &rill_system::plan::Plan) -> Result<(), Error> {
        use rill_system::plan::{Redirect, Stage};
        use std::ffi::CString;
        self.charge(plan.stages.capacity().saturating_mul(size_of::<Stage>()))?;
        for stage in &plan.stages {
            self.charge(stage.argv.capacity().saturating_mul(size_of::<CString>()))?;
            for argument in &stage.argv {
                self.charge(argument.as_bytes_with_nul().len())?;
            }
            self.charge(
                stage
                    .redirects
                    .capacity()
                    .saturating_mul(size_of::<Redirect>()),
            )?;
            for redirect in &stage.redirects {
                match redirect {
                    Redirect::Read(path) | Redirect::Write { path, .. } => {
                        self.charge(path.capacity())?;
                    }
                    Redirect::ErrorToOutput => {}
                }
            }
            if let Some(cwd) = &stage.cwd {
                self.charge(cwd.capacity())?;
            }
            self.charge(
                stage
                    .environment
                    .len()
                    .saturating_mul(size_of::<(CString, CString)>()),
            )?;
            for (key, value) in &stage.environment {
                self.charge(key.as_bytes_with_nul().len())?;
                self.charge(value.as_bytes_with_nul().len())?;
            }
            self.charge(stage.accepted_codes.capacity())?;
        }
        Ok(())
    }
}
