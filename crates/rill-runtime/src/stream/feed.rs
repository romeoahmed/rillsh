//! The input branch of through: retain one language chunk and send bounded slices.
use super::{Node, Source, merge::Merge, node};
use crate::{Error, value::Value};
use gc_arena::{Gc, Mutation, RefLock};
use rill_system::resources::SourceId;

pub(super) enum Pull<'gc> {
    Input(Node<'gc>),
    Send(SourceId, bytes::Bytes),
}

pub(super) fn opened<'gc>(mc: &Mutation<'gc>, input: Node<'gc>, reply: Value<'gc>) -> Value<'gc> {
    let Value::List(values) = reply else {
        unreachable!("through launch response")
    };
    let [Value::Stream(output), Value::Stream(writer)] = values.as_slice() else {
        unreachable!("through endpoints")
    };
    let Some(Source::External(sink)) = writer.borrow_mut(mc).source.take() else {
        unreachable!("through writer")
    };
    let owner = output.borrow().owner;
    let output = node(
        mc,
        output.borrow_mut(mc).source.take().expect("through output"),
    );
    let feed = node(
        mc,
        Source::Feed {
            input,
            sink,
            chunk: None,
            offset: 0,
        },
    );
    super::token(
        mc,
        owner,
        Source::Merge(Gc::new(mc, RefLock::new(Merge::through(output, feed)))),
    )
}

pub(super) fn pull<'gc>(mc: &Mutation<'gc>, source: Node<'gc>) -> Pull<'gc> {
    let mut data = source.borrow_mut(mc);
    let Some(Source::Feed {
        input,
        sink,
        chunk,
        offset,
    }) = &mut *data
    else {
        unreachable!("through feeder")
    };
    if let Some(Value::Bytes(bytes)) = chunk {
        let end = offset
            .saturating_add(rill_system::IO_CHUNK_BYTES)
            .min(bytes.0.len());
        let bytes = bytes.0.slice(*offset..end);
        *offset = end;
        return Pull::Send(*sink, bytes);
    }
    Pull::Input(*input)
}

pub(super) fn received<'gc>(
    mc: &Mutation<'gc>,
    source: Node<'gc>,
    item: Option<Value<'gc>>,
) -> Result<Option<SourceId>, Error> {
    let mut data = source.borrow_mut(mc);
    let Some(Source::Feed {
        sink,
        chunk,
        offset,
        ..
    }) = &mut *data
    else {
        unreachable!("through input continuation")
    };
    match item {
        Some(value @ Value::Bytes(_)) => {
            *chunk = Some(value);
            *offset = 0;
            Ok(None)
        }
        Some(_) => Err(Error::type_error("through requires Bytes chunks")),
        None => {
            let sink = *sink;
            *data = Some(Source::Empty);
            Ok(Some(sink))
        }
    }
}

pub(super) fn sink(source: Node<'_>) -> SourceId {
    let Some(Source::Feed { sink, .. }) = *source.borrow() else {
        unreachable!("through writer continuation")
    };
    sink
}

pub(super) fn written<'gc>(mc: &Mutation<'gc>, source: Node<'gc>, open: bool) -> Vec<SourceId> {
    let mut data = source.borrow_mut(mc);
    let Some(Source::Feed {
        input,
        sink,
        chunk,
        offset,
    }) = &mut *data
    else {
        unreachable!("through write continuation")
    };
    if open {
        if let Some(Value::Bytes(bytes)) = chunk
            && *offset == bytes.0.len()
        {
            *chunk = None;
            *offset = 0;
        }
        Vec::new()
    } else {
        let mut keys = super::machine::resources(*input);
        keys.push(*sink);
        *data = Some(Source::Empty);
        keys
    }
}

pub(super) fn connect<'gc>(
    mc: &Mutation<'gc>,
    plan: &rill_system::plan::Plan,
    input: Value<'gc>,
    owner: u64,
) -> Result<super::Outcome<'gc>, Error> {
    use rill_system::plan::Redirect;
    if plan.stages.first().is_some_and(|stage| {
        stage
            .redirects
            .iter()
            .any(|redirect| matches!(redirect, Redirect::Read(_)))
    }) || plan.stages.last().is_some_and(|stage| {
        stage.redirects.iter().any(|redirect| {
            matches!(
                redirect,
                Redirect::Write {
                    stream: rill_system::plan::Output::Stdout,
                    ..
                }
            )
        })
    }) {
        return Err(Error::type_error(
            "through cannot redirect its input or output",
        ));
    }
    Ok(super::Outcome::Task(Box::new(super::Task::new(
        super::consume(mc, input, owner)?,
        super::machine::Consumer::Through(Some(plan.clone())),
    ))))
}
