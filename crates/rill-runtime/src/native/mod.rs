//! Private representation primitives; ordinary Rill functions own composition.
pub mod budget;
pub mod collection;
pub mod json;
pub mod plan;
mod pure;
pub mod report;
pub mod service;
pub mod sort;
mod text;
pub mod work;
use crate::value::Value;
use gc_arena::Collect;
pub use pure::{apply, items};
use std::collections::HashMap;
pub use text::display_path;
pub use text::native_path;

/// Unary VM entry points. Higher arities are expressed by library closures.
#[derive(Clone, Copy, Collect)]
#[collect(require_static)]
pub enum Intrinsic {
    Pure,
    Bytes,
    Path,
    Attempt,
    Raise,
    Data,
    Stream,
    Process,
    Host(&'static str),
}

pub fn bindings<'gc>() -> HashMap<String, Value<'gc>> {
    let primitives = [
        ("__pure", Intrinsic::Pure),
        ("bytes", Intrinsic::Bytes),
        ("path", Intrinsic::Path),
        ("attempt", Intrinsic::Attempt),
        ("raise", Intrinsic::Raise),
        ("__data", Intrinsic::Data),
        ("__stream", Intrinsic::Stream),
        ("__process", Intrinsic::Process),
    ];
    primitives
        .into_iter()
        .map(|(name, primitive)| (name.into(), Value::Native(primitive)))
        .chain(
            [
                "run",
                "execute",
                "start",
                "wait",
                "check",
                "fg",
                "bg",
                "cancel",
                "jobs",
                "cd",
                "pwd",
                "exit",
                "exit_force",
            ]
            .into_iter()
            .map(|name| (name.into(), Value::Native(Intrinsic::Host(name)))),
        )
        .collect()
}
