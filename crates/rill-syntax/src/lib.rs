//! Rill source syntax, independent of evaluation and Unix services.

pub mod ast;
pub mod completion;
pub mod format;
mod parser;
pub mod token;
pub use parser::{Diagnostic, parse};
mod validate;
