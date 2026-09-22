//! Single-consumer stream graphs and resumable consumers; OS resources stay outside the heap.
mod feed;
mod lines;
mod machine;
mod merge;
pub mod producer;
use crate::{
    Error, native,
    value::{List, Value},
};
use gc_arena::{Collect, Gc, Mutation, RefLock};
pub use machine::{Step, Task};

#[derive(Collect)]
#[collect(no_drop)]
pub struct Token<'gc> {
    pub owner: u64,
    pub source: Option<Source<'gc>>,
}
pub fn token<'gc>(mc: &Mutation<'gc>, owner: u64, source: Source<'gc>) -> Value<'gc> {
    Value::Stream(Gc::new(
        mc,
        RefLock::new(Token {
            owner,
            source: Some(source),
        }),
    ))
}

pub type Node<'gc> = Gc<'gc, RefLock<Option<Source<'gc>>>>;

#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
pub enum Source<'gc> {
    External(#[collect(require_static)] rill_system::resources::SourceId),
    Chunks(Value<'gc>),
    Items(List<'gc>),
    Unfold {
        step: Value<'gc>,
        state: Value<'gc>,
    },
    Map {
        input: Node<'gc>,
        transform: Value<'gc>,
    },
    FlatMap {
        input: Node<'gc>,
        transform: Value<'gc>,
        inner: Option<Node<'gc>>,
        owner: u64,
    },
    Filter {
        input: Node<'gc>,
        predicate: Value<'gc>,
    },
    Take {
        input: Node<'gc>,
        remaining: usize,
    },
    Drop {
        input: Node<'gc>,
        remaining: usize,
    },
    Lines {
        input: Node<'gc>,
        buffer: Gc<'gc, RefLock<lines::Lines<'gc>>>,
    },
    Merge(Gc<'gc, RefLock<merge::Merge<'gc>>>),
    Zip(Gc<'gc, RefLock<Zip<'gc>>>),
    Feed {
        input: Node<'gc>,
        #[collect(require_static)]
        sink: rill_system::resources::SourceId,
        chunk: Option<Value<'gc>>,
        offset: usize,
    },
    Producer(producer::Handle<'gc>),
    Empty,
}
#[derive(Collect)]
#[collect(no_drop)]
pub struct Zip<'gc> {
    inputs: Vec<Node<'gc>>,
    tuple: Vec<Value<'gc>>,
    ended: bool,
}
pub enum Outcome<'gc> {
    Host(crate::host::Request),
    Value(Value<'gc>),
    Task(Box<Task<'gc>>),
    Producer(producer::Protocol<'gc>),
}
fn node<'gc>(mc: &Mutation<'gc>, source: Source<'gc>) -> Node<'gc> {
    Gc::new(mc, RefLock::new(Some(source)))
}
fn consume<'gc>(mc: &Mutation<'gc>, value: Value<'gc>, owner: u64) -> Result<Node<'gc>, Error> {
    let Value::Stream(stream) = value else {
        return Err(Error::type_error("operation requires Stream"));
    };
    if stream.borrow().owner != owner {
        return Err(Error::new(
            "ResourceEscape",
            "stream belongs to another resource scope",
        ));
    }
    let source = stream.borrow_mut(mc).source.take().ok_or_else(|| {
        Error::new(
            "StreamConsumed",
            "stream ownership has already been transferred",
        )
    })?;
    Ok(node(mc, source))
}
pub fn display<'gc>(mc: &Mutation<'gc>, value: Value<'gc>, owner: u64) -> Result<Task<'gc>, Error> {
    Ok(Task::new(
        consume(mc, value, owner)?,
        machine::Consumer::Display,
    ))
}
fn callable(value: Value<'_>) -> Result<(), Error> {
    if matches!(
        value,
        Value::Function(_) | Value::Native(_) | Value::Constructor(_)
    ) {
        Ok(())
    } else {
        Err(Error::type_error("stream callback requires Function"))
    }
}
pub fn apply<'gc>(
    mc: &Mutation<'gc>,
    argument: Value<'gc>,
    owner: u64,
) -> Result<Outcome<'gc>, Error> {
    let request = native::items(argument)?;
    let [Value::String(name), args @ ..] = request.as_slice() else {
        return Err(Error::type_error("invalid stream request"));
    };
    if let Some(request) = host(name, args)? {
        return Ok(Outcome::Host(request));
    }
    let source = match (name.as_str(), args) {
        ("produce", [protocol]) => {
            return Ok(Outcome::Producer(producer::Protocol::new(*protocol)?));
        }
        ("through", [Value::Plan(plan), input]) => {
            return feed::connect(mc, &plan.0, *input, owner);
        }
        ("merge", [Value::List(values)]) => Source::Merge(Gc::new(
            mc,
            RefLock::new(merge::Merge::new(consume_inputs(
                mc,
                values.as_slice(),
                owner,
            )?)),
        )),
        ("zip", [Value::List(values)]) => {
            let inputs = consume_inputs(mc, values.as_slice(), owner)?;
            if inputs.is_empty() {
                Source::Empty
            } else {
                Source::Zip(Gc::new(
                    mc,
                    RefLock::new(Zip {
                        inputs,
                        tuple: Vec::new(),
                        ended: false,
                    }),
                ))
            }
        }
        ("chunks", [value @ Value::Bytes(_)]) => Source::Chunks(*value),
        ("items", [Value::List(values)]) => Source::Items(*values),
        ("unfold", [step, state]) => {
            callable(*step)?;
            Source::Unfold {
                step: *step,
                state: *state,
            }
        }
        ("flat_map", [transform, stream]) => {
            callable(*transform)?;
            Source::FlatMap {
                input: consume(mc, *stream, owner)?,
                transform: *transform,
                inner: None,
                owner,
            }
        }
        ("map" | "filter", [callback, stream]) => {
            callable(*callback)?;
            let input = consume(mc, *stream, owner)?;
            if name.as_str() == "map" {
                Source::Map {
                    input,
                    transform: *callback,
                }
            } else {
                Source::Filter {
                    input,
                    predicate: *callback,
                }
            }
        }
        ("lines" | "split_nul", [options, stream]) => {
            let buffer = lines::Lines::new(*options, name.as_str() == "split_nul")?;
            let input = consume(mc, *stream, owner)?;
            Source::Lines {
                input,
                buffer: Gc::new(mc, RefLock::new(buffer)),
            }
        }
        ("take" | "drop", [Value::Int(count), stream]) => {
            let remaining = usize::try_from(*count)
                .map_err(|_| Error::type_error("count must be nonnegative"))?;
            let input = consume(mc, *stream, owner)?;
            if name.as_str() == "take" {
                Source::Take { input, remaining }
            } else {
                Source::Drop { input, remaining }
            }
        }
        ("close", [stream]) => {
            return Ok(Outcome::Task(Box::new(Task::new(
                consume(mc, *stream, owner)?,
                machine::Consumer::Close,
            ))));
        }
        _ => return consumer(mc, name, args, owner),
    };
    Ok(Outcome::Value(token(mc, owner, source)))
}
fn host(name: &str, args: &[Value<'_>]) -> Result<Option<crate::host::Request>, Error> {
    use crate::host::Request;
    Ok(match (name, args) {
        ("stdin", []) => Some(Request::Stdin),
        ("stream", [Value::Plan(plan)]) => Some(Request::Stream(plan.0.clone())),
        ("read_file", [path]) => Some(Request::OpenFile {
            path: native::native_path(*path)?.to_path_buf(),
            append: None,
        }),
        ("files", [path]) => Some(Request::Files(native::native_path(*path)?.to_path_buf())),
        _ => None,
    })
}
fn consumer<'gc>(
    mc: &Mutation<'gc>,
    name: &str,
    args: &[Value<'gc>],
    owner: u64,
) -> Result<Outcome<'gc>, Error> {
    use machine::Consumer;
    let (stream, consumer) = match (name, args) {
        ("collect", [options, stream]) => {
            let (max_items, max_bytes) = limits(*options)?;
            (
                *stream,
                Consumer::Collect {
                    values: Vec::new(),
                    max_items,
                    budget: native::budget::Budget::new(max_bytes),
                },
            )
        }
        ("collect_bytes", [options, stream]) => (
            *stream,
            Consumer::Bytes {
                bytes: Vec::new(),
                limit: native::service::byte_limit(*options)?,
            },
        ),
        ("write_file" | "append_file", [path, stream]) => (
            *stream,
            Consumer::File {
                path: Some(native::native_path(*path)?.to_path_buf()),
                append: name == "append_file",
                sink: None,
            },
        ),
        ("write" | "write_stderr", [stream]) => (
            *stream,
            Consumer::Write {
                stderr: name == "write_stderr",
            },
        ),
        ("fold" | "fold_until", [step, initial, stream]) => {
            callable(*step)?;
            (
                *stream,
                Consumer::Fold {
                    step: *step,
                    accumulator: *initial,
                    until: name == "fold_until",
                },
            )
        }
        ("each", [effect, stream]) => {
            callable(*effect)?;
            (*stream, Consumer::Each(*effect))
        }
        _ => {
            return Err(Error::new(
                "UnsupportedOperation",
                format!("invalid or unavailable stream operation '{name}'"),
            ));
        }
    };
    Ok(Outcome::Task(Box::new(Task::new(
        consume(mc, stream, owner)?,
        consumer,
    ))))
}
fn limits(options: Value<'_>) -> Result<(usize, usize), Error> {
    let Value::Record(fields) = options else {
        return Err(Error::type_error("options require Record"));
    };
    let (mut items, mut bytes) = (1_000_000, 64 * 1024 * 1024);
    for (name, value) in fields.iter() {
        let Value::Int(n) = value else {
            return Err(Error::type_error("limits require Int"));
        };
        let n = usize::try_from(*n).map_err(|_| Error::type_error("limits must be nonnegative"))?;
        match name.as_str() {
            "max_items" => items = n,
            "max_bytes" => bytes = n,
            _ => return Err(Error::type_error(format!("unknown limit '{name}'"))),
        }
    }
    Ok((items, bytes))
}

fn consume_inputs<'gc>(
    mc: &Mutation<'gc>,
    values: &[Value<'gc>],
    owner: u64,
) -> Result<Vec<Node<'gc>>, Error> {
    let mut seen = std::collections::HashSet::new();
    for value in values {
        let Value::Stream(stream) = value else {
            return Err(Error::type_error(
                "stream composition requires a List of Streams",
            ));
        };
        if stream.borrow().owner != owner {
            return Err(Error::new(
                "ResourceEscape",
                "stream belongs to another resource scope",
            ));
        }
        if stream.borrow().source.is_none() {
            return Err(Error::new(
                "StreamConsumed",
                "stream ownership has already been transferred",
            ));
        }
        if !seen.insert(Gc::as_ptr(*stream)) {
            return Err(Error::new(
                "StreamConsumed",
                "a stream cannot occur twice in the same composition",
            ));
        }
    }
    values
        .iter()
        .map(|value| consume(mc, *value, owner))
        .collect()
}
