//! Incremental editor parsing for Rill Shell, independently of execution.
//!
//! ```
//! let mut parser = tree_sitter::Parser::new();
//! parser.set_language(&tree_sitter_rill::LANGUAGE.into()).unwrap();
//! let tree = parser.parse("fn double x = x * 2", None).unwrap();
//! assert!(!tree.root_node().has_error());
//! ```
use tree_sitter_language::LanguageFn;
unsafe extern "C" {
    fn tree_sitter_rill() -> *const ();
}
/// Generated Tree-sitter language descriptor.
// SAFETY: the generated parser exports a static language with Tree-sitter's ABI.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_rill) };
/// Generated node metadata for editor integrations.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");
/// Syntax highlighting captures.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");
/// Lexical scope and binding captures.
pub const LOCALS_QUERY: &str = include_str!("../../queries/locals.scm");
