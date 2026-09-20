//! Unix resources owned outside the language heap.

pub mod helper;
pub mod job;
pub mod plan;
pub mod report;
pub mod resources;
pub mod source;
pub mod terminal;
pub mod writer;

mod input;
mod output;
mod process_source;
mod sys;

pub use sys::glob;

/// Maximum queued transport payload; readers and producers use the same chunk size.
/// This bounds bytes in flight, not the total size of a language Bytes value.
pub const IO_CHUNK_BYTES: usize = 16 * 1024;
