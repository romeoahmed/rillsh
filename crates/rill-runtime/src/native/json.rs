//! Strict JSON conversion through borrowed `RawValue` spans and Serde serializers.
// Containers are expanded iteratively, so a user depth option cannot exhaust the host stack.
use crate::{Error, operations::list, value::Value};
use gc_arena::{Collect, Gc, Mutation};
use serde::{
    Deserializer, Serialize,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{
    fmt,
    io::{self, Write},
    ops::Range,
};

/// Traced conversion state. No borrowed input span survives an arena mutation.
#[derive(Collect)]
#[collect(no_drop)]
pub enum Work<'gc> {
    Decode(Decoder<'gc>),
    Encode(Encoder<'gc>),
}
impl<'gc> Work<'gc> {
    pub fn new(name: &str, options: Value<'gc>, input: Value<'gc>) -> Result<Self, Error> {
        let (bytes, depth) = limits(options)?;
        match name {
            "from_json" => {
                let source = source(input)?;
                if source.len() > bytes {
                    return Err(Error::new(
                        "LimitExceeded",
                        "JSON input exceeds the byte limit",
                    ));
                }
                let root: &RawValue =
                    serde_json::from_slice(source).map_err(|error| decode_error(&error))?;
                Ok(Self::Decode(Decoder {
                    input,
                    tasks: vec![Task::Value(span(source, root), 1)],
                    values: Vec::new(),
                    max_depth: depth,
                }))
            }
            "to_json" => Ok(Self::Encode(Encoder {
                output: Output {
                    bytes: Vec::new(),
                    limit: bytes,
                },
                tasks: vec![Emit::Value(input, 1)],
                path: Vec::new(),
                max_depth: depth,
            })),
            _ => Err(Error::type_error("unknown JSON operation")),
        }
    }
    pub fn advance(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<Option<Value<'gc>>, Error> {
        match self {
            Self::Decode(work) => work.advance(mc, fuel),
            Self::Encode(work) => work.advance(mc, fuel),
        }
    }
}
fn source(input: Value<'_>) -> Result<&[u8], Error> {
    match input {
        Value::String(text) => Ok(Gc::as_ref(text).as_bytes()),
        Value::Bytes(bytes) => Ok(&Gc::as_ref(bytes).0),
        _ => Err(Error::type_error("JSON input requires String or Bytes")),
    }
}
// Serde's borrowed RawValue is a subslice of this input. Retain offsets, not addresses.
fn span(input: &[u8], raw: &RawValue) -> Range<usize> {
    let start = raw.get().as_ptr().addr() - input.as_ptr().addr();
    start..start + raw.get().len()
}
fn limits(options: Value<'_>) -> Result<(usize, usize), Error> {
    let Value::Record(fields) = options else {
        return Err(Error::type_error("JSON options require Record"));
    };
    let mut bytes = 64 * 1024 * 1024;
    let mut depth = 256;
    for (name, value) in fields.iter() {
        let Value::Int(value) = value else {
            return Err(Error::type_error("JSON limits require Int"));
        };
        let value = usize::try_from(*value)
            .map_err(|_| Error::type_error("JSON limits must be nonnegative"))?;
        match name.as_str() {
            "max_bytes" => bytes = value,
            "max_depth" if value > 0 => depth = value,
            _ => return Err(Error::type_error(format!("invalid JSON limit '{name}'"))),
        }
    }
    Ok((bytes, depth))
}

/// One borrowed container at a time; this is not an intermediate JSON value tree.
enum Container<'a> {
    List(Vec<&'a RawValue>),
    Record(indexmap::IndexMap<String, &'a RawValue>),
}
struct Children;
impl<'de> Visitor<'de> for Children {
    type Value = Container<'de>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON array or object")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<Self::Value, A::Error> {
        let mut children = Vec::new();
        while let Some(value) = input.next_element()? {
            children.push(value);
        }
        Ok(Container::List(children))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Self::Value, A::Error> {
        let mut children = indexmap::IndexMap::new();
        while let Some(name) = input.next_key::<String>()? {
            match children.entry(name) {
                indexmap::map::Entry::Vacant(entry) => {
                    entry.insert(input.next_value()?);
                }
                indexmap::map::Entry::Occupied(entry) => {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate object key '{}'",
                        entry.key()
                    )));
                }
            }
        }
        Ok(Container::Record(children))
    }
}
#[derive(Collect)]
#[collect(require_static)]
enum Task {
    Value(Range<usize>, usize),
    List(usize),
    Record(Vec<String>),
}
#[derive(Collect)]
#[collect(no_drop)]
pub struct Decoder<'gc> {
    input: Value<'gc>,
    tasks: Vec<Task>,
    values: Vec<Value<'gc>>,
    max_depth: usize,
}
impl<'gc> Decoder<'gc> {
    fn advance(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<Option<Value<'gc>>, Error> {
        let input = source(self.input)?;
        let Self {
            tasks,
            values,
            max_depth,
            ..
        } = self;
        while *fuel != 0 {
            let Some(task) = tasks.pop() else {
                return Ok(values.pop());
            };
            *fuel -= 1;
            match task {
                Task::Value(raw, depth) => {
                    if depth > *max_depth {
                        return Err(Error::new(
                            "LimitExceeded",
                            "JSON nesting exceeds the depth limit",
                        ));
                    }
                    let source = std::str::from_utf8(&input[raw])
                        .map_err(|error| Error::new("DecodeError", error.to_string()))?;
                    match source.as_bytes()[0] {
                        b'[' | b'{' => {
                            let container = serde_json::Deserializer::from_str(source)
                                .deserialize_any(Children)
                                .map_err(|error| decode_error(&error))?;
                            match container {
                                Container::List(children) => {
                                    tasks.push(Task::List(children.len()));
                                    tasks.extend(
                                        children.into_iter().rev().map(|child| {
                                            Task::Value(span(input, child), depth + 1)
                                        }),
                                    );
                                }
                                Container::Record(children) => {
                                    let (keys, children): (Vec<_>, Vec<_>) =
                                        children.into_iter().unzip();
                                    tasks.push(Task::Record(keys));
                                    tasks.extend(
                                        children.into_iter().rev().map(|child| {
                                            Task::Value(span(input, child), depth + 1)
                                        }),
                                    );
                                }
                            }
                        }
                        b'"' => values.push(Value::string(
                            mc,
                            serde_json::from_str::<String>(source)
                                .map_err(|error| decode_error(&error))?,
                        )),
                        b'n' => values.push(Value::Null),
                        b't' | b'f' => values.push(Value::Bool(source == "true")),
                        _ => values.push(number(source)?),
                    }
                }
                Task::List(count) => {
                    let children = values.split_off(values.len() - count);
                    values.push(list(mc, children));
                }
                Task::Record(keys) => {
                    let children = values.drain(values.len() - keys.len()..);
                    let fields = keys.into_iter().zip(children).collect();
                    values.push(Value::Record(crate::heap::record(mc, fields)));
                }
            }
            if crate::heap::should_yield(mc, *fuel) {
                break;
            }
        }
        Ok(None)
    }
}
fn number<'gc>(source: &str) -> Result<Value<'gc>, Error> {
    if source.contains(['.', 'e', 'E']) {
        source
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(Value::Float)
            .ok_or_else(Error::arithmetic)
    } else {
        source
            .parse()
            .map(Value::Int)
            .map_err(|_| Error::arithmetic())
    }
}
fn decode_error(error: &serde_json::Error) -> Error {
    Error::new("DecodeError", error.to_string())
}

struct Output {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "JSON output exceeds the byte limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Collect)]
#[collect(no_drop)]
enum Emit<'gc> {
    Value(Value<'gc>, usize),
    Children(Value<'gc>, usize, usize),
}
#[derive(Collect)]
#[collect(no_drop)]
pub struct Encoder<'gc> {
    #[collect(require_static)]
    output: Output,
    tasks: Vec<Emit<'gc>>,
    // Ancestor containers keep field names rooted without copying or escaping them.
    path: Vec<(Value<'gc>, usize)>,
    max_depth: usize,
}
impl<'gc> Encoder<'gc> {
    fn advance(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<Option<Value<'gc>>, Error> {
        while *fuel != 0 {
            let Some(task) = self.tasks.pop() else {
                return Ok(Some(Value::bytes(
                    mc,
                    std::mem::take(&mut self.output.bytes),
                )));
            };
            *fuel -= 1;
            match task {
                Emit::Value(value, depth) => {
                    if depth > self.max_depth {
                        return Err(Error::new(
                            "LimitExceeded",
                            format!("JSON depth limit exceeded at {}", self.path()),
                        ));
                    }
                    match value {
                        Value::Null => scalar(&mut self.output, &Option::<bool>::None)?,
                        Value::Bool(v) => scalar(&mut self.output, &v)?,
                        Value::Int(v) => scalar(&mut self.output, &v)?,
                        Value::Float(v) => scalar(&mut self.output, &v)?,
                        Value::String(v) => scalar(&mut self.output, v.as_str())?,
                        Value::List(_) | Value::Record(_) => {
                            self.punctuation(if matches!(value, Value::List(_)) {
                                b"["
                            } else {
                                b"{"
                            })?;
                            self.tasks.push(Emit::Children(value, 0, depth));
                        }
                        _ => {
                            return Err(Error::type_error(format!(
                                "cannot encode {} at {}",
                                value.kind(),
                                self.path()
                            )));
                        }
                    }
                }
                Emit::Children(value, index, depth) => {
                    self.path.truncate(depth - 1);
                    // Keep only one child and its continuation, regardless of container width.
                    let child = match value {
                        Value::List(items) => items.as_slice().get(index).copied(),
                        Value::Record(fields) => {
                            if let Some((name, child)) = fields.get_index(index) {
                                if index != 0 {
                                    self.punctuation(b",")?;
                                }
                                scalar(&mut self.output, name)?;
                                self.punctuation(b":")?;
                                Some(*child)
                            } else {
                                None
                            }
                        }
                        _ => unreachable!("JSON container cursor"),
                    };
                    if let Some(child) = child {
                        if index != 0 && matches!(value, Value::List(_)) {
                            self.punctuation(b",")?;
                        }
                        self.path.push((value, index));
                        self.tasks.push(Emit::Children(value, index + 1, depth));
                        self.tasks.push(Emit::Value(child, depth + 1));
                    } else {
                        self.punctuation(if matches!(value, Value::List(_)) {
                            b"]"
                        } else {
                            b"}"
                        })?;
                    }
                }
            }
        }
        Ok(None)
    }
    fn path(&self) -> String {
        use std::fmt::Write;
        let mut path = String::from("$");
        for &(container, index) in &self.path {
            match container {
                Value::List(_) => write!(path, "[{index}]").expect("String write"),
                Value::Record(fields) => {
                    let (name, _) = fields.get_index(index).expect("JSON field cursor");
                    let key = serde_json::to_string(name).expect("String serialization");
                    write!(path, "[{key}]").expect("String write");
                }
                _ => unreachable!("JSON path container"),
            }
        }
        path
    }
    fn punctuation(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.output
            .write_all(bytes)
            .map_err(|error| Error::new("LimitExceeded", error.to_string()))
    }
}
fn scalar(output: &mut Output, value: &(impl Serialize + ?Sized)) -> Result<(), Error> {
    value
        .serialize(&mut serde_json::Serializer::new(output))
        .map_err(|error| {
            Error::new(
                if error.is_io() {
                    "LimitExceeded"
                } else {
                    "TypeError"
                },
                error.to_string(),
            )
        })
}
