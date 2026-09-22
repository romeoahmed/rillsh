//! Owned bytecode and lexical captures; instructions never contain unrooted GC pointers.
use crate::value::Value;
use gc_arena::{Gc, Mutation, RefLock};
use indexmap::IndexSet;
use rill_syntax::{
    ast::{Binary, Literal, Pattern, Unary},
    token::Span,
};
use std::rc::Rc;

/// Source retained by closures so later diagnostics identify the original definition.
#[derive(Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub text: String,
}

pub struct Code {
    pub source: Rc<Source>,
    pub instructions: Vec<(Instruction, Span)>,
    pub constants: Vec<Literal>,
    pub layouts: Vec<Rc<IndexSet<String>>>,
    pub context: Option<usize>,
}

/// A traced cache belongs to a live executable or closure, never a global code registry.
pub type Constants<'gc> = Gc<'gc, RefLock<Vec<Option<Value<'gc>>>>>;

impl Code {
    pub fn cache<'gc>(&self, mc: &Mutation<'gc>) -> Constants<'gc> {
        crate::heap::charge(
            mc,
            self.constants
                .len()
                .saturating_mul(size_of::<Option<Value<'gc>>>()),
        );
        Gc::new(mc, RefLock::new(vec![None; self.constants.len()]))
    }
}

pub struct FunctionCode {
    pub documentation: Option<String>,
    pub name: Option<String>,
    pub parameters: Vec<Pattern>,
    pub captures: Vec<Capture>,
    pub body: Rc<Code>,
}

pub struct Capture {
    pub name: String,
    /// Destination in the function's base layout.
    pub slot: usize,
    /// Resolved against the frame that creates the closure.
    pub source: Reference,
}

#[derive(Clone)]
pub enum Reference {
    /// Scope depth is relative to the current frame, never the caller's frame.
    Local { scope: usize, slot: usize },
    /// Entry/module snapshot lookup; function free names become captured slots.
    Global(String),
}

pub enum Instruction {
    DriveStream,
    DriveCleanup,
    Boundary { last: bool },
    BeginPlan,
    BeginStage,
    Argument { spread: bool },
    Redirect(rill_syntax::token::Redirect),
    EndPlan,
    RunPlan,
    Import { path: String, name: String },
    Export(Vec<(String, Vec<String>)>),
    Constant(usize),
    Load(Reference),
    Pop,
    List(usize),
    Record(Vec<String>),
    Field(String),
    Index,
    Unary(Unary),
    Binary(Binary),
    Enter(usize),
    Leave,
    Bind(Pattern),
    Closure(Rc<FunctionCode>),
    Functions(Vec<Rc<FunctionCode>>),
    Struct(String, Vec<String>),
    Enum(String, Vec<(String, Vec<String>)>),
    TryBind(Pattern, usize),
    NoMatch,
    Call { tail: bool },
    Branch(usize),
    Jump(usize),
    Bool,
    Swap,
    Return,
}
