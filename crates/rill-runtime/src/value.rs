//! Immutable language values. Only closure knot-tying uses traced interior mutation.
use crate::{Error, code::FunctionCode, native::Intrinsic};
use gc_arena::{Collect, Gc, Mutation, RefLock};
use indexmap::IndexMap;
use std::rc::Rc;

/// Ordered fields with randomized lookup for user-controlled keys.
pub type Record<'gc> = IndexMap<String, Value<'gc>>;

/// A Rill value valid only during its owning arena's mutation callback.
#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
pub enum Value<'gc> {
    Unit,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(Gc<'gc, String>),
    Bytes(Gc<'gc, Blob>),
    Path(Gc<'gc, NativePath>),
    Native(Intrinsic),
    Stream(Gc<'gc, RefLock<crate::stream::Token<'gc>>>),
    Job(i64),
    Plan(Gc<'gc, JobPlan>),
    Budget(Gc<'gc, RefLock<crate::native::budget::Budget>>),
    List(List<'gc>),
    Record(Gc<'gc, Record<'gc>>),
    Function(Gc<'gc, Closure<'gc>>),
    Constructor(Gc<'gc, Descriptor>),
    Adt(Gc<'gc, Adt<'gc>>),
}

/// Reusable process description; launched resources have separate ownership.
#[derive(Collect)]
#[collect(require_static)]
pub struct JobPlan(pub rill_system::plan::Plan);

/// Shared immutable bytes and their original backing capacity for retained-value budgets.
#[derive(Collect)]
#[collect(require_static)]
pub struct Blob(pub bytes::Bytes, pub(crate) usize);

/// Byte-preserving Unix path, validated to contain no NUL.
#[derive(Collect)]
#[collect(require_static)]
pub struct NativePath(pub std::path::PathBuf);

/// Shared suffixes make recursive list matching independent of suffix length.
#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
pub struct List<'gc> {
    values: Gc<'gc, Vec<Value<'gc>>>,
    start: usize,
}

impl<'gc> List<'gc> {
    #[must_use]
    pub fn new(mc: &Mutation<'gc>, values: Vec<Value<'gc>>) -> Self {
        crate::heap::charge(
            mc,
            values.capacity().saturating_mul(size_of::<Value<'gc>>()),
        );
        Self {
            values: Gc::new(mc, values),
            start: 0,
        }
    }
    pub(crate) const fn storage(self) -> Gc<'gc, Vec<Value<'gc>>> {
        self.values
    }
    #[must_use]
    pub fn as_slice(&self) -> &[Value<'gc>] {
        &self.values[self.start..]
    }
    #[must_use]
    pub fn suffix(self, count: usize) -> Option<Self> {
        (count <= self.as_slice().len()).then(|| Self {
            start: self.start + count,
            ..self
        })
    }
}

#[derive(Collect)]
#[collect(no_drop)]
pub struct Closure<'gc> {
    #[collect(require_static)]
    pub(crate) code: Rc<FunctionCode>,
    pub(crate) parameter: usize,
    pub(crate) constants: crate::code::Constants<'gc>,
    pub(crate) environment: Gc<'gc, RefLock<crate::scope::Scope<'gc>>>,
    pub(crate) arguments: Option<Gc<'gc, Arguments<'gc>>>,
}

/// Partial applications share captures and earlier bindings instead of copying frame slots.
#[derive(Collect)]
#[collect(no_drop)]
pub(crate) struct Arguments<'gc> {
    pub previous: Option<Gc<'gc, Self>>,
    pub bindings: Bindings<'gc>,
}
#[derive(Collect)]
#[collect(no_drop)]
pub(crate) enum Bindings<'gc> {
    One((usize, Value<'gc>)),
    Many(Vec<(usize, Value<'gc>)>),
}
impl<'gc> Bindings<'gc> {
    pub fn as_slice(&self) -> &[(usize, Value<'gc>)] {
        match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
        }
    }
    pub const fn capacity(&self) -> usize {
        match self {
            Self::One(_) => 0,
            Self::Many(values) => values.capacity(),
        }
    }
    pub fn iter(&self) -> std::slice::Iter<'_, (usize, Value<'gc>)> {
        self.as_slice().iter()
    }
}
impl<'gc> Closure<'gc> {
    pub(crate) fn bind(
        &self,
        mc: &Mutation<'gc>,
        argument: Value<'gc>,
    ) -> Result<Bindings<'gc>, Error> {
        use rill_syntax::ast::Pattern;
        let environment = self.environment.borrow();
        let slot = |name: &str| {
            environment
                .layout
                .get_index_of(name)
                .expect("parameter slot")
        };
        match &self.code.parameters[self.parameter] {
            Pattern::Bind(name) => return Ok(Bindings::One((slot(name), argument))),
            Pattern::Ignore => return Ok(Bindings::Many(Vec::new())),
            _ => {}
        }
        let resolve = |name: &str| {
            let slot = environment.layout.get_index_of(name);
            let mut arguments = self.arguments;
            while let Some(bound) = arguments {
                if let Some((_, value)) = bound
                    .bindings
                    .iter()
                    .find(|(index, _)| Some(*index) == slot)
                {
                    return Ok(*value);
                }
                arguments = bound.previous;
            }
            environment
                .get(name)
                .copied()
                .ok_or_else(|| Error::new("NameError", format!("unknown constructor '{name}'")))
        };
        let bindings = crate::pattern::bind(
            mc,
            &self.code.parameters[self.parameter],
            argument,
            &resolve,
        )?;
        Ok(Bindings::Many(
            bindings
                .into_iter()
                .map(|(name, value)| (slot(name), value))
                .collect(),
        ))
    }
    pub(crate) fn frame(&self, bindings: &Bindings<'gc>) -> crate::scope::Scope<'gc> {
        let mut environment = self.environment.borrow().clone();
        if let Some(last) = self.arguments {
            if last.previous.is_none() {
                for &(slot, value) in last.bindings.as_slice() {
                    environment.slots[slot] = Some(value);
                }
            } else {
                let mut ordered = Vec::with_capacity(self.parameter);
                let mut arguments = Some(last);
                while let Some(bound) = arguments {
                    ordered.push(bound);
                    arguments = bound.previous;
                }
                // Later parameters may shadow earlier ones; replay in application order.
                for bound in ordered.into_iter().rev() {
                    for &(slot, value) in bound.bindings.as_slice() {
                        environment.slots[slot] = Some(value);
                    }
                }
            }
        }
        for &(slot, value) in bindings.as_slice() {
            environment.slots[slot] = Some(value);
        }
        environment
    }
}
/// Nominal constructor identity is its unique traced descriptor, never its spelling.
#[derive(Collect)]
#[collect(require_static)]
pub struct Descriptor {
    pub name: String,
    pub fields: Vec<String>,
}

#[derive(Collect)]
#[collect(no_drop)]
pub struct Adt<'gc> {
    pub descriptor: Gc<'gc, Descriptor>,
    pub fields: Gc<'gc, Record<'gc>>,
}

impl<'gc> Value<'gc> {
    /// Documentation follows a function through aliases and partial application.
    #[must_use]
    pub fn documentation(self) -> Option<&'gc str> {
        if let Self::Function(function) = self {
            Gc::as_ref(function).code.documentation.as_deref()
        } else {
            None
        }
    }
    /// Describe a value without invoking it or consuming a resource.
    #[must_use]
    pub fn help(self) -> String {
        let signature = self.signature().map_or_else(
            || self.kind().into(),
            |parameters| format!("Function: {parameters}"),
        );
        if let Some(documentation) = self.documentation() {
            format!("{signature}\n{documentation}")
        } else {
            signature
        }
    }

    /// Describe remaining parameters without calling a function or retaining its environment.
    #[must_use]
    pub fn signature(self) -> Option<String> {
        use rill_syntax::ast::{Literal, Pattern};
        let short = |text: &str| {
            let end = text.floor_char_boundary(64);
            format!(
                "{}{}",
                &text[..end],
                if end < text.len() { "…" } else { "" }
            )
        };
        let parameter = |pattern: &Pattern| match pattern {
            Pattern::Bind(name) => short(name),
            Pattern::Ignore => "_".into(),
            Pattern::Literal(Literal::Unit) => "()".into(),
            Pattern::Literal(_) => "literal".into(),
            Pattern::List(..) => "[items]".into(),
            Pattern::Record(..) => "{fields}".into(),
            Pattern::Constructor(path, _) => path
                .iter()
                .take(8)
                .map(|name| short(name))
                .collect::<Vec<_>>()
                .join("."),
        };
        Some(match self {
            Self::Function(function) => {
                let parameters = &function.code.parameters[function.parameter..];
                let mut text = parameters
                    .iter()
                    .take(8)
                    .map(parameter)
                    .collect::<Vec<_>>()
                    .join(" ");
                if parameters.len() > 8 {
                    text.push_str(" …");
                }
                text
            }
            Self::Constructor(descriptor) => {
                let fields = descriptor
                    .fields
                    .iter()
                    .take(8)
                    .map(|field| short(field))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{{{fields}{}}}",
                    if descriptor.fields.len() > 8 {
                        ", …"
                    } else {
                        ""
                    }
                )
            }
            Self::Native(intrinsic) => match intrinsic {
                Intrinsic::Attempt => "action",
                Intrinsic::Raise => "error",
                Intrinsic::Host("run" | "execute" | "start") => "plan",
                Intrinsic::Host("wait" | "fg" | "bg" | "cancel") => "job",
                Intrinsic::Host("check") => "report",
                Intrinsic::Host("jobs" | "pwd") => "()",
                Intrinsic::Host("cd") => "path",
                Intrinsic::Host("exit" | "exit_force") => "code",
                _ => "value",
            }
            .into(),
            _ => return None,
        })
    }
    #[must_use]
    pub fn string(mc: &Mutation<'gc>, text: impl Into<String>) -> Self {
        let text = text.into();
        crate::heap::charge(mc, text.capacity());
        Self::String(Gc::new(mc, text))
    }
    #[must_use]
    pub fn bytes(mc: &Mutation<'gc>, bytes: Vec<u8>) -> Self {
        let capacity = bytes.capacity();
        crate::heap::charge(mc, capacity);
        Self::Bytes(Gc::new(mc, Blob(bytes.into(), capacity)))
    }
    /// Construct a native path without Unicode conversion.
    ///
    /// # Errors
    /// Rejects NUL bytes, which Unix path operations cannot represent.
    pub fn path(mc: &Mutation<'gc>, path: std::path::PathBuf) -> Result<Self, Error> {
        use std::os::unix::ffi::OsStrExt;
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(Error::new("TypeError", "Path cannot contain NUL"));
        }
        crate::heap::charge(mc, path.capacity());
        Ok(Self::Path(Gc::new(mc, NativePath(path))))
    }
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Stream(_) => "Stream",
            Self::Budget(_) => "Internal",
            Self::Plan(_) => "JobPlan",
            Self::Job(_) => "Job",
            Self::Unit => "Unit",
            Self::Null => "Null",
            Self::Bool(_) => "Bool",
            Self::Int(_) => "Int",
            Self::Float(_) => "Float",
            Self::String(_) => "String",
            Self::Bytes(_) => "Bytes",
            Self::Path(_) => "Path",
            Self::List(_) => "List",
            Self::Record(_) => "Record",
            Self::Function(_) | Self::Constructor(_) | Self::Native(_) => "Function",
            Self::Adt(_) => "ADT",
        }
    }
    /// Check comparability independently of whether an earlier field already differs.
    /// This synchronous helper shares the VM's comparison algorithm; language operators
    /// use its resumable form so long comparisons participate in scheduling.
    ///
    /// # Errors
    /// Rejects noncomparable values anywhere in either operand.
    pub fn equals(self, other: Self) -> Result<bool, Error> {
        if let Some(equal) = crate::equality::immediate(self, other)? {
            return Ok(equal);
        }
        let mut comparison = crate::equality::Equality::new(self, other, false);
        loop {
            if let Some(equal) = comparison.step()? {
                return Ok(equal);
            }
        }
    }
    pub(crate) fn persistent(self) -> Result<(), Error> {
        self.within_scope(None)
    }
    pub(crate) fn within_scope(self, owner: Option<u64>) -> Result<(), Error> {
        Self::check_scope([self], owner)
    }
    pub(crate) fn persistent_all(values: impl IntoIterator<Item = Self>) -> Result<(), Error> {
        Self::check_scope(values, None)
    }
    fn check_scope(
        values: impl IntoIterator<Item = Self>,
        owner: Option<u64>,
    ) -> Result<(), Error> {
        let mut pending: Vec<_> = values.into_iter().collect();
        let mut seen = std::collections::HashSet::new();
        while let Some(value) = pending.pop() {
            match value {
                Self::Stream(token) if Some(token.borrow().owner) != owner => {
                    return Err(Error::new(
                        "ResourceEscape",
                        "a scoped stream cannot escape its statement; consume it inside do, collect its data, or retain a source function",
                    ));
                }
                Self::List(values) => {
                    if seen.insert(Gc::as_ptr(values.storage()).addr()) {
                        pending.extend_from_slice(&values.storage());
                    }
                }
                Self::Record(fields) => {
                    if seen.insert(Gc::as_ptr(fields).addr()) {
                        pending.extend(fields.values().copied());
                    }
                }
                Self::Adt(value) => pending.push(Self::Record(value.fields)),
                Self::Function(closure) => {
                    if seen.insert(Gc::as_ptr(closure.environment).addr()) {
                        pending.extend(closure.environment.borrow().values().copied());
                    }
                    let mut arguments = closure.arguments;
                    while let Some(bound) = arguments {
                        if !seen.insert(Gc::as_ptr(bound).addr()) {
                            break;
                        }
                        pending.extend(bound.bindings.iter().map(|(_, value)| *value));
                        arguments = bound.previous;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// Project a record field without invoking user code.
    ///
    /// # Errors
    /// Fails for non-records or absent fields.
    pub fn field(self, name: &str) -> Result<Self, Error> {
        let fields = match self {
            Self::Record(fields) => fields,
            Self::Adt(value) => value.fields,
            _ => {
                return Err(Error::type_error(
                    "field access requires a Record or nominal value",
                ));
            }
        };
        fields
            .get(name)
            .copied()
            .ok_or_else(|| Error::new("MissingField", format!("record has no field '{name}'")))
    }
}

/// Concise, escaped display text; Unit has no automatic presentation.
#[must_use]
#[expect(
    clippy::unnecessary_debug_formatting,
    reason = "Path rendering must escape terminal control characters and preserve invalid UTF-8 as visible escapes"
)]
pub fn summary(value: Value<'_>) -> Option<String> {
    if let Some(signature) = value.signature() {
        return Some(format!("<Function: {signature}>"));
    }
    Some(match value {
        Value::Unit => return None,
        Value::Job(id) => format!("<Job {id}>"),
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::String(value) => crate::presentation::preview(format_args!("{:?}", value.as_str())),
        Value::Path(value) => crate::presentation::preview(format_args!("{:?}", value.0)),
        Value::Bytes(_) | Value::List(_) | Value::Record(_) | Value::Adt(_) => {
            crate::presentation::compact(value)
        }
        _ => format!("<{}>", value.kind()),
    })
}
