//! Resumable structural equality over immutable values, without unfolding shared DAGs.
//!
//! Validate both operands before comparing: neither identity nor an early mismatch
//! may hide a noncomparable leaf. Union-find records outstanding child obligations,
//! avoiding a Cartesian product of equivalent subgraphs. Cursors visit bounded edge
//! batches; byte comparisons use bounded slices. Roots keep every address key alive
//! across collection. No traversal state escapes its arena or survives the operation.
use crate::{Error, value::Value};
use gc_arena::{Collect, Gc};
use std::{collections::HashMap, os::unix::ffi::OsStrExt};

const BYTE_QUANTUM: usize = 4096;
const EDGE_QUANTUM: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Identity {
    List(usize, usize),
    Record(usize),
}

#[derive(Default)]
struct Classes {
    indices: HashMap<Identity, usize>,
    parents: Vec<usize>,
    ranks: Vec<usize>,
}

impl Classes {
    fn insert(&mut self, key: Identity) -> bool {
        let next = self.parents.len();
        if let std::collections::hash_map::Entry::Vacant(entry) = self.indices.entry(key) {
            entry.insert(next);
            self.parents.push(next);
            self.ranks.push(0);
            true
        } else {
            false
        }
    }

    fn root(&mut self, mut index: usize) -> usize {
        while self.parents[index] != index {
            self.parents[index] = self.parents[self.parents[index]];
            index = self.parents[index];
        }
        index
    }

    /// A new union still requires comparing its children; an existing union does not.
    fn join(&mut self, left: Identity, right: Identity) -> bool {
        let mut left = self.root(self.indices[&left]);
        let mut right = self.root(self.indices[&right]);
        if left == right {
            return false;
        }
        if self.ranks[left] < self.ranks[right] {
            std::mem::swap(&mut left, &mut right);
        }
        self.parents[right] = left;
        self.ranks[left] += usize::from(self.ranks[left] == self.ranks[right]);
        true
    }
}

#[derive(Collect)]
#[collect(no_drop)]
enum Work<'gc> {
    Validate(Value<'gc>),
    Children(Value<'gc>, usize),
    Compare(Value<'gc>, Value<'gc>),
    Elements(Value<'gc>, Value<'gc>, usize),
    Bytes(Value<'gc>, Value<'gc>, usize),
}

#[derive(Collect)]
#[collect(no_drop)]
pub struct Equality<'gc> {
    roots: [Value<'gc>; 2],
    pending: Vec<Work<'gc>>,
    #[collect(require_static)]
    classes: Classes,
    negate: bool,
}

impl<'gc> Equality<'gc> {
    pub fn new(left: Value<'gc>, right: Value<'gc>, negate: bool) -> Self {
        Self {
            roots: [left, right],
            pending: vec![
                Work::Compare(left, right),
                Work::Validate(right),
                Work::Validate(left),
            ],
            classes: Classes::default(),
            negate,
        }
    }

    /// Perform a bounded batch of edge visits or byte comparisons.
    /// None means more work remains.
    pub fn step(&mut self) -> Result<Option<bool>, Error> {
        let Some(work) = self.pending.pop() else {
            return Ok(Some(!self.negate));
        };
        let equal = match work {
            Work::Validate(value) => {
                validate_leaf(value)?;
                if let Some(key) = identity(value) {
                    if self.classes.insert(key) {
                        self.pending.push(Work::Children(value, 0));
                    }
                } else if let Value::Adt(adt) = value {
                    self.pending.push(Work::Validate(Value::Record(adt.fields)));
                }
                true
            }
            Work::Children(value, index) => {
                let end = length(value).min(index.saturating_add(EDGE_QUANTUM));
                if end < length(value) {
                    self.pending.push(Work::Children(value, end));
                }
                for index in index..end {
                    let child = child(value, index).expect("bounded aggregate cursor");
                    validate_leaf(child)?;
                    if aggregate(child) {
                        self.pending.push(Work::Validate(child));
                    }
                }
                true
            }
            Work::Compare(left, right) => self.compare(left, right)?,
            Work::Elements(left, right, index) => self.elements(left, right, index),
            Work::Bytes(left, right, offset) => {
                let a = bytes(left).expect("byte comparison");
                let b = bytes(right).expect("byte comparison");
                let end = offset + (a.len() - offset).min(BYTE_QUANTUM);
                if end < a.len() {
                    self.pending.push(Work::Bytes(left, right, end));
                }
                a[offset..end] == b[offset..end]
            }
        };
        if !equal {
            self.pending.clear();
            return Ok(Some(self.negate));
        }
        Ok(None)
    }

    /// Spend the caller's remaining work budget without repeated bytecode dispatch.
    pub fn advance(&mut self, fuel: &mut usize) -> Result<Option<bool>, Error> {
        while *fuel != 0 {
            *fuel -= 1;
            if let Some(equal) = self.step()? {
                return Ok(Some(equal));
            }
        }
        Ok(None)
    }

    fn compare(&mut self, left: Value<'gc>, right: Value<'gc>) -> Result<bool, Error> {
        if let Some(equal) = immediate(left, right)? {
            return Ok(equal);
        }
        match (left, right) {
            (Value::List(a), Value::List(b)) if a.as_slice().len() == b.as_slice().len() => {}
            (Value::Record(a), Value::Record(b)) if a.len() == b.len() => {}
            (Value::Adt(a), Value::Adt(b)) => {
                if !Gc::ptr_eq(a.descriptor, b.descriptor) {
                    return Ok(false);
                }
                self.pending.push(Work::Compare(
                    Value::Record(a.fields),
                    Value::Record(b.fields),
                ));
                return Ok(true);
            }
            _ if left.kind() == right.kind() && bytes(left).is_some() => {
                self.pending.push(Work::Bytes(left, right, 0));
                return Ok(true);
            }
            _ => return Ok(false),
        }
        if self
            .classes
            .join(identity(left).unwrap(), identity(right).unwrap())
        {
            self.pending.push(Work::Elements(left, right, 0));
        }
        Ok(true)
    }

    fn elements(&mut self, left: Value<'gc>, right: Value<'gc>, index: usize) -> bool {
        let end = length(left).min(index.saturating_add(EDGE_QUANTUM));
        if end < length(left) {
            self.pending.push(Work::Elements(left, right, end));
        }
        for index in index..end {
            let a = child(left, index).expect("bounded aggregate cursor");
            let b = match (left, right) {
                (Value::Record(a), Value::Record(b)) => {
                    let (name, _) = a.get_index(index).expect("record cursor");
                    let Some(value) = b.get(name) else {
                        return false;
                    };
                    *value
                }
                _ => child(right, index).expect("equal list lengths"),
            };
            match immediate(a, b).expect("both graphs passed validation") {
                Some(false) => return false,
                Some(true) => {}
                None => self.pending.push(Work::Compare(a, b)),
            }
        }
        true
    }
}

fn identity(value: Value<'_>) -> Option<Identity> {
    match value {
        // Different suffixes of one backing allocation are different logical values.
        Value::List(list) => Some(Identity::List(
            Gc::as_ptr(list.storage()).addr(),
            list.as_slice().len(),
        )),
        Value::Record(record) => Some(Identity::Record(Gc::as_ptr(record).addr())),
        _ => None,
    }
}

fn child(value: Value<'_>, index: usize) -> Option<Value<'_>> {
    match value {
        Value::List(list) => list.as_slice().get(index).copied(),
        Value::Record(record) => record.get_index(index).map(|(_, value)| *value),
        _ => unreachable!("aggregate cursor"),
    }
}

fn length(value: Value<'_>) -> usize {
    match value {
        Value::List(list) => list.as_slice().len(),
        Value::Record(record) => record.len(),
        _ => unreachable!("aggregate cursor"),
    }
}

const fn aggregate(value: Value<'_>) -> bool {
    matches!(value, Value::List(_) | Value::Record(_) | Value::Adt(_))
}

fn bytes(value: Value<'_>) -> Option<&[u8]> {
    match value {
        Value::String(text) => Some(Gc::as_ref(text).as_bytes()),
        Value::Bytes(blob) => Some(&Gc::as_ref(blob).0),
        Value::Path(path) => Some(Gc::as_ref(path).0.as_os_str().as_bytes()),
        _ => None,
    }
}

fn validate_leaf(value: Value<'_>) -> Result<(), Error> {
    if matches!(
        value,
        Value::Function(_)
            | Value::Constructor(_)
            | Value::Native(_)
            | Value::Stream(_)
            | Value::Budget(_)
            | Value::Job(_)
            | Value::Plan(_)
    ) {
        return Err(Error::type_error(format!(
            "{} values cannot be compared",
            value.kind()
        )));
    }
    Ok(())
}

/// Avoid traversal allocation for scalars and small byte values. Aggregates must first
/// validate both complete graphs, even when their kinds or identities differ.
#[expect(
    clippy::float_cmp,
    reason = "Rill Float equality is exact IEEE equality"
)]
pub fn immediate<'gc>(left: Value<'gc>, right: Value<'gc>) -> Result<Option<bool>, Error> {
    match (left, right) {
        (Value::Unit, Value::Unit) | (Value::Null, Value::Null) => return Ok(Some(true)),
        (Value::Bool(a), Value::Bool(b)) => return Ok(Some(a == b)),
        (Value::Int(a), Value::Int(b)) => return Ok(Some(a == b)),
        (Value::Float(a), Value::Float(b)) => return Ok(Some(a == b)),
        _ => {}
    }
    if aggregate(left) || aggregate(right) {
        return flat_lists(left, right);
    }
    validate_leaf(left)?;
    validate_leaf(right)?;
    if left.kind() != right.kind() {
        return Ok(Some(false));
    }
    if let (Some(a), Some(b)) = (bytes(left), bytes(right)) {
        return Ok(if a.len() != b.len() {
            Some(false)
        } else if a.as_ptr() == b.as_ptr() {
            Some(true)
        } else if a.len() <= BYTE_QUANTUM {
            Some(a == b)
        } else {
            None
        });
    }
    unreachable!("validated scalar kinds")
}

// Common short scalar lists need neither address tables nor a suspended frame.
// Validate every leaf before using length or element mismatch as an answer.
fn flat_lists<'gc>(left: Value<'gc>, right: Value<'gc>) -> Result<Option<bool>, Error> {
    let (Value::List(a), Value::List(b)) = (left, right) else {
        return Ok(None);
    };
    let (a, b) = (a.as_slice(), b.as_slice());
    if a.len() > EDGE_QUANTUM
        || b.len() > EDGE_QUANTUM
        || a.iter().chain(b).any(|value| aggregate(*value))
    {
        return Ok(None);
    }
    for value in a.iter().chain(b) {
        validate_leaf(*value)?;
    }
    if a.len() != b.len() {
        return Ok(Some(false));
    }
    for (&left, &right) in a.iter().zip(b) {
        match immediate(left, right)? {
            Some(true) => {}
            result => return Ok(result),
        }
    }
    Ok(Some(true))
}
