//! An explicit pull stack suspends through the ordinary VM callback and host protocols.
use super::{
    Node, Source,
    producer::{self, Callback, Handle, Reason},
};
use crate::{Error, host::Request, native::budget::Budget, operations::list, value::Value};
use gc_arena::{Collect, Gc, Mutation, RefLock};

#[derive(Collect)]
#[collect(no_drop)]
pub enum Consumer<'gc> {
    One,
    Through(#[collect(require_static)] Option<rill_system::plan::Plan>),
    Collect {
        values: Vec<Value<'gc>>,
        max_items: usize,
        #[collect(require_static)]
        budget: Budget,
    },
    Bytes {
        bytes: Vec<u8>,
        limit: usize,
    },
    Fold {
        step: Value<'gc>,
        accumulator: Value<'gc>,
        until: bool,
    },
    Each(Value<'gc>),
    Write,
    Display,
    Close,
}
#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
enum Action<'gc> {
    Pull(Node<'gc>),
    Produced(Handle<'gc>, Callback),
    External(Node<'gc>),
    Lined(Node<'gc>),
    Zipped(Node<'gc>),
    Merged(Node<'gc>),
    Opened,
    FeedInput(Node<'gc>),
    FeedQueued(Node<'gc>),
    FeedWritten(Node<'gc>),
    Map(Value<'gc>),
    Mapped,
    Filter {
        source: Node<'gc>,
        predicate: Value<'gc>,
    },
    Filtered {
        source: Node<'gc>,
        item: Value<'gc>,
    },
    Dropped(Node<'gc>),
    Unfolded {
        source: Node<'gc>,
        step: Value<'gc>,
    },
    Folded,
    Discard,
}
#[derive(Collect)]
#[collect(no_drop)]
pub struct Task<'gc> {
    pub(super) source: Node<'gc>,
    consumer: Consumer<'gc>,
    actions: Vec<Action<'gc>>,
    item: Option<Value<'gc>>,
    reply: Option<Value<'gc>>,
    #[collect(require_static)]
    pub(super) error: Option<Box<Error>>,
    pub awaiting: bool,
    finished: Option<Value<'gc>>,
    #[collect(require_static)]
    owned: Vec<rill_system::resources::SourceId>,
    producers: Vec<Handle<'gc>>,
    merges: Box<[Gc<'gc, RefLock<super::merge::Merge<'gc>>>]>,
}
pub enum Step<'gc> {
    Yield,
    Callback(
        crate::vm::callback::Handle<'gc>,
        Option<(Option<u64>, Value<'gc>, Value<'gc>)>,
    ),
    DiscardCallbacks(Vec<crate::vm::callback::Handle<'gc>>),
    Call(Value<'gc>, Value<'gc>),
    ScopedCall(u64, Value<'gc>, Value<'gc>),
    Finalize(Vec<Handle<'gc>>, Reason),
    Host(Request),
    Done(Value<'gc>),
}
impl<'gc> Task<'gc> {
    pub fn new(source: Node<'gc>, consumer: Consumer<'gc>) -> Self {
        let actions = if matches!(consumer, Consumer::Close | Consumer::Through(_)) {
            Vec::new()
        } else {
            vec![Action::Pull(source)]
        };
        let mut owned = Vec::new();
        let mut producers = Vec::new();
        let mut merges = Vec::new();
        // Merge branches borrow their graph from the enclosing consumer. Scanning it
        // on every single-item pull adds work without transferring any ownership.
        if !matches!(consumer, Consumer::One) {
            walk(source, |source| match source {
                Source::External(key) | Source::Feed { sink: key, .. } => owned.push(key),
                Source::Producer(producer) => producers.push(producer),
                Source::Merge(merge) => merges.push(merge),
                _ => {}
            });
        }
        Self {
            source,
            consumer,
            actions,
            item: None,
            reply: None,
            error: None,
            awaiting: false,
            finished: None,
            owned,
            producers,
            merges: merges.into_boxed_slice(),
        }
    }
    pub(super) fn pull_one(source: Node<'gc>) -> Self {
        Self::new(source, Consumer::One)
    }
    pub const fn resume(&mut self, value: Value<'gc>) {
        self.reply = Some(value);
        self.awaiting = false;
    }
    pub fn resume_operation(
        &mut self,
        mc: &Mutation<'gc>,
        index: usize,
        result: Result<crate::host::Response, Error>,
    ) -> Result<(), Error> {
        self.awaiting = false;
        if let Some(Action::Merged(source)) = self.actions.last().copied() {
            self.actions.pop();
            if let Some(Source::Merge(merge)) = *source.borrow() {
                merge.borrow_mut(mc).resume_operation(mc, index, result);
                self.actions.push(Action::Pull(source));
                return Ok(());
            }
        }
        Err(Error::new(
            "RuntimeError",
            "callback reply has no merge continuation",
        ))
    }
    pub fn resume_read_error(&mut self, mc: &Mutation<'gc>, index: usize, error: Error) {
        self.awaiting = false;
        if let Some(Action::Merged(source)) = self.actions.last().copied() {
            self.actions.pop();
            if let Some(Source::Merge(merge)) = *source.borrow() {
                merge.borrow_mut(mc).resume_error(mc, index, error);
                self.actions.push(Action::Pull(source));
                return;
            }
        }
        self.error = Some(Box::new(error));
    }
    pub fn advance(&mut self, mc: &Mutation<'gc>) -> Result<Step<'gc>, Error> {
        if let Some(error) = self.error.take() {
            return Err(*error);
        }
        if let Consumer::Through(plan) = &mut self.consumer
            && let Some(plan) = plan.take()
        {
            self.actions.push(Action::Opened);
            self.awaiting = true;
            return Ok(Step::Host(Request::Through(plan)));
        }
        for _ in 0..64 {
            if let Some(action) = self.actions.pop() {
                if let Some(step) = self.action(mc, action)? {
                    self.awaiting = true;
                    return Ok(step);
                }
            } else if let Some(item) = self.item.take() {
                if let Some(step) = self.consume(mc, item)? {
                    self.awaiting = true;
                    return Ok(step);
                }
            } else {
                let value = self.finished.take().unwrap_or_else(|| self.finish(mc));
                if !self.merges.is_empty() {
                    self.finished = Some(value);
                    self.actions.push(Action::Discard);
                    self.awaiting = true;
                    let callbacks = self.callbacks();
                    self.merges = Box::default();
                    return Ok(Step::DiscardCallbacks(callbacks));
                }
                if !self.producers.is_empty() {
                    self.finished = Some(value);
                    self.actions.push(Action::Discard);
                    self.awaiting = true;
                    let reason = if matches!(self.consumer, Consumer::Close) {
                        Reason::Closed
                    } else {
                        Reason::Cutoff
                    };
                    return Ok(Step::Finalize(std::mem::take(&mut self.producers), reason));
                }
                if !self.owned.is_empty() {
                    self.finished = Some(value);
                    self.actions.push(Action::Discard);
                    self.awaiting = true;
                    return Ok(Step::Host(Request::Close(std::mem::take(&mut self.owned))));
                }
                return Ok(Step::Done(value));
            }
        }
        Ok(Step::Yield)
    }
    fn action(
        &mut self,
        mc: &Mutation<'gc>,
        action: Action<'gc>,
    ) -> Result<Option<Step<'gc>>, Error> {
        match action {
            Action::Produced(producer, callback) => return self.produced(mc, producer, callback),
            Action::Opened => {
                self.finished = Some(super::feed::opened(
                    mc,
                    self.source,
                    self.reply.take().expect("through launch"),
                ));
                self.owned.clear();
                self.producers.clear();
            }
            Action::FeedInput(source) => {
                if let Some(key) = super::feed::received(mc, source, self.item.take())? {
                    self.actions.push(Action::Discard);
                    return Ok(Some(Step::Host(Request::Send { key, bytes: None })));
                }
                self.actions.push(Action::Pull(source));
            }
            Action::FeedQueued(source) => {
                self.reply.take();
                self.actions.push(Action::FeedWritten(source));
                return Ok(Some(Step::Host(Request::ReadSources {
                    keys: vec![super::feed::sink(source)],
                    wait: true,
                })));
            }
            Action::FeedWritten(source) => return self.feed_written(mc, source),
            Action::External(source) => {
                let reply = self.reply.take().expect("source response");
                let (_, values) = super::merge::ready(reply)?;
                let Value::List(values) = values else {
                    return Err(Error::type_error("invalid source response"));
                };
                self.item = values.as_slice().first().copied();
                if self.item.is_none() {
                    *source.borrow_mut(mc) = Some(Source::Empty);
                }
            }
            Action::Lined(source) => {
                let Some(Source::Lines { buffer, .. }) = *source.borrow() else {
                    unreachable!("line continuation")
                };
                let mut buffer = buffer.borrow_mut(mc);
                match self.item.take() {
                    Some(value @ Value::Bytes(_)) => buffer.chunk = Some(value),
                    Some(_) => return Err(Error::type_error("lines requires Bytes chunks")),
                    None => buffer.ended = true,
                }
                self.actions.push(Action::Pull(source));
            }
            Action::Pull(source) => return self.pull(mc, source),
            Action::Zipped(source) => self.zipped(mc, source),
            Action::Merged(source) => {
                let Some(Source::Merge(merge)) = *source.borrow() else {
                    unreachable!("merge continuation")
                };
                merge
                    .borrow_mut(mc)
                    .resume(mc, self.reply.take().expect("merge response"))?;
                self.actions.push(Action::Pull(source));
            }
            Action::Map(transform) => {
                if let Some(item) = self.item.take() {
                    self.actions.push(Action::Mapped);
                    return Ok(Some(Step::Call(transform, item)));
                }
            }
            Action::Mapped => self.item = self.reply.take(),
            Action::Filter { source, predicate } => {
                if let Some(item) = self.item.take() {
                    self.actions.push(Action::Filtered { source, item });
                    return Ok(Some(Step::Call(predicate, item)));
                }
            }
            Action::Filtered { source, item } => match self.reply.take() {
                Some(Value::Bool(true)) => self.item = Some(item),
                Some(Value::Bool(false)) => self.actions.push(Action::Pull(source)),
                _ => return Err(Error::type_error("filter predicate must return Bool")),
            },
            Action::Dropped(source) => {
                let mut data = source.borrow_mut(mc);
                let Some(Source::Drop { remaining, .. }) = &mut *data else {
                    unreachable!("drop continuation")
                };
                if self.item.take().is_some() {
                    *remaining -= 1;
                    self.actions.push(Action::Pull(source));
                }
            }
            Action::Unfolded { source, step } => self.unfolded(mc, source, step)?,
            Action::Folded => self.folded(mc)?,
            Action::Discard => {
                self.reply.take();
            }
        }
        Ok(None)
    }
    fn produced(
        &mut self,
        mc: &Mutation<'gc>,
        producer: Handle<'gc>,
        callback: Callback,
    ) -> Result<Option<Step<'gc>>, Error> {
        let reply = self.reply.take().expect("producer callback result");
        match callback {
            Callback::Acquire => {
                producer.borrow_mut(mc).acquired(reply)?;
                self.actions
                    .push(Action::Pull(super::node(mc, Source::Producer(producer))));
            }
            Callback::Step => {
                self.item = producer.borrow_mut(mc).stepped(reply)?;
                if self.item.is_none() {
                    self.actions.push(Action::Discard);
                    return Ok(Some(Step::Finalize(vec![producer], Reason::Exhausted)));
                }
            }
        }
        Ok(None)
    }
    fn feed_written(
        &mut self,
        mc: &Mutation<'gc>,
        source: Node<'gc>,
    ) -> Result<Option<Step<'gc>>, Error> {
        let (_, response) = super::merge::ready(self.reply.take().expect("write response"))?;
        let Value::List(values) = response else {
            unreachable!("write response list")
        };
        let [Value::Bool(open)] = values.as_slice() else {
            unreachable!("write acknowledgment")
        };
        let keys = super::feed::written(mc, source, *open);
        self.actions.push(Action::Pull(source));
        if !keys.is_empty() {
            self.actions.push(Action::Discard);
            return Ok(Some(Step::Host(Request::Close(keys))));
        }
        Ok(None)
    }
    fn folded(&mut self, mc: &Mutation<'gc>) -> Result<(), Error> {
        let reply = self.reply.take().expect("fold callback result");
        let Consumer::Fold {
            accumulator, until, ..
        } = &mut self.consumer
        else {
            unreachable!("fold continuation")
        };
        let (stop, value) = if *until {
            let Value::List(values) = reply else {
                return Err(Error::type_error("fold_until requires Control"));
            };
            let [Value::Bool(stop), value] = values.as_slice() else {
                return Err(Error::type_error("fold_until requires Control"));
            };
            (*stop, *value)
        } else {
            (false, reply)
        };
        *accumulator = value;
        if stop {
            self.actions.clear();
            *self.source.borrow_mut(mc) = Some(Source::Empty);
        }
        Ok(())
    }
    fn zipped(&mut self, mc: &Mutation<'gc>, source: Node<'gc>) {
        let Some(Source::Zip(zip)) = *source.borrow() else {
            unreachable!("zip continuation")
        };
        let mut zip = zip.borrow_mut(mc);
        if let Some(item) = self.item.take() {
            zip.tuple.push(item);
            if zip.tuple.len() == zip.inputs.len() {
                self.item = Some(list(mc, std::mem::take(&mut zip.tuple)));
            } else {
                self.actions.extend([
                    Action::Zipped(source),
                    Action::Pull(zip.inputs[zip.tuple.len()]),
                ]);
            }
        } else {
            zip.tuple.clear();
            *source.borrow_mut(mc) = Some(Source::Empty);
        }
    }
    fn unfolded(
        &mut self,
        mc: &Mutation<'gc>,
        source: Node<'gc>,
        step: Value<'gc>,
    ) -> Result<(), Error> {
        let reply = self.reply.take().expect("unfold callback result");
        let Value::List(values) = reply else {
            return Err(Error::type_error(
                "unfold step requires Option of [item, next_state]",
            ));
        };
        match values.as_slice() {
            [] => *source.borrow_mut(mc) = Some(Source::Empty),
            [item, state] => {
                *source.borrow_mut(mc) = Some(Source::Unfold {
                    step,
                    state: *state,
                });
                self.item = Some(*item);
            }
            _ => {
                return Err(Error::type_error(
                    "unfold step requires Option of [item, next_state]",
                ));
            }
        }
        Ok(())
    }
    fn pull(&mut self, mc: &Mutation<'gc>, source: Node<'gc>) -> Result<Option<Step<'gc>>, Error> {
        let data = source
            .borrow()
            .as_ref()
            .copied()
            .ok_or_else(|| Error::new("StreamConsumed", "stream source is not available"))?;
        match data {
            Source::Producer(producer) => return self.pull_producer(mc, producer),
            Source::Feed { .. } => match super::feed::pull(mc, source) {
                super::feed::Pull::Input(input) => self
                    .actions
                    .extend([Action::FeedInput(source), Action::Pull(input)]),
                super::feed::Pull::Send(key, bytes) => {
                    self.actions.push(Action::FeedQueued(source));
                    return Ok(Some(Step::Host(Request::Send {
                        key,
                        bytes: Some(bytes),
                    })));
                }
            },
            Source::External(key) => {
                self.actions.push(Action::External(source));
                return Ok(Some(Step::Host(Request::ReadSources {
                    keys: vec![key],
                    wait: true,
                })));
            }
            Source::Lines { input, buffer } => match buffer.borrow_mut(mc).poll(mc)? {
                super::lines::Poll::Item(value) => self.item = Some(value),
                super::lines::Poll::End => {
                    self.item = None;
                    *source.borrow_mut(mc) = Some(Source::Empty);
                }
                super::lines::Poll::Continue => self.actions.push(Action::Pull(source)),
                super::lines::Poll::NeedInput => self
                    .actions
                    .extend([Action::Lined(source), Action::Pull(input)]),
            },
            Source::Merge(merge) => match merge.borrow_mut(mc).advance(mc)? {
                Step::Done(value) => {
                    let Value::List(values) = value else {
                        unreachable!("merge item")
                    };
                    self.item = values.as_slice().first().copied();
                }
                Step::Yield => self.actions.push(Action::Pull(source)),
                step => {
                    self.actions.push(Action::Merged(source));
                    return Ok(Some(step));
                }
            },
            Source::Zip(zip) => {
                let input = zip.borrow().inputs[0];
                self.actions
                    .extend([Action::Zipped(source), Action::Pull(input)]);
            }
            Source::Empty => self.item = None,
            Source::Items(values) => {
                self.item = values.as_slice().first().copied();
                *source.borrow_mut(mc) =
                    Some(values.suffix(1).map_or(Source::Empty, Source::Items));
            }
            Source::Chunks(value) => {
                let Value::Bytes(bytes) = value else {
                    unreachable!("byte source")
                };
                self.item = (!bytes.0.is_empty()).then_some(value);
                *source.borrow_mut(mc) = Some(Source::Empty);
            }
            Source::Unfold { step, state } => {
                self.actions.push(Action::Unfolded { source, step });
                return Ok(Some(Step::Call(step, state)));
            }
            Source::Map { input, transform } => {
                self.actions
                    .extend([Action::Map(transform), Action::Pull(input)]);
            }
            Source::Filter { input, predicate } => {
                self.actions
                    .extend([Action::Filter { source, predicate }, Action::Pull(input)]);
            }
            Source::Take { input, remaining } => {
                if remaining == 0 {
                    self.item = None;
                    *source.borrow_mut(mc) = Some(Source::Empty);
                } else {
                    *source.borrow_mut(mc) = Some(Source::Take {
                        input,
                        remaining: remaining - 1,
                    });
                    self.actions.push(Action::Pull(input));
                }
            }
            Source::Drop { input, remaining } => {
                if remaining != 0 {
                    self.actions.push(Action::Dropped(source));
                }
                self.actions.push(Action::Pull(input));
            }
        }
        Ok(None)
    }
    fn pull_producer(
        &mut self,
        mc: &Mutation<'gc>,
        producer: Handle<'gc>,
    ) -> Result<Option<Step<'gc>>, Error> {
        let mut state = producer.borrow_mut(mc);
        if let Some((callback, function, argument)) = state.pull()? {
            self.actions.push(Action::Produced(producer, callback));
            return Ok(Some(Step::ScopedCall(state.scope, function, argument)));
        }
        self.item = None;
        Ok(None)
    }
    fn consume(
        &mut self,
        mc: &Mutation<'gc>,
        item: Value<'gc>,
    ) -> Result<Option<Step<'gc>>, Error> {
        self.actions.push(Action::Pull(self.source));
        match &mut self.consumer {
            Consumer::One => {
                self.actions.clear();
                self.finished = Some(list(mc, vec![item]));
            }
            Consumer::Collect {
                values,
                max_items,
                budget,
            } => {
                if values.len() == *max_items {
                    return Err(Error::new("LimitExceeded", "stream exceeds the item limit"));
                }
                budget.value(item)?;
                values.push(item);
            }
            Consumer::Bytes { bytes, limit } => {
                let Value::Bytes(chunk) = item else {
                    return Err(Error::type_error("collect_bytes requires Bytes chunks"));
                };
                if chunk.0.len() > limit.saturating_sub(bytes.len()) {
                    return Err(Error::new("LimitExceeded", "stream exceeds the byte limit"));
                }
                bytes.extend_from_slice(&chunk.0);
            }
            Consumer::Fold {
                step, accumulator, ..
            } => {
                self.actions.push(Action::Folded);
                return Ok(Some(Step::Call(*step, list(mc, vec![*accumulator, item]))));
            }
            Consumer::Each(effect) => {
                self.actions.push(Action::Discard);
                return Ok(Some(Step::Call(*effect, item)));
            }
            Consumer::Close | Consumer::Through(_) => {
                unreachable!("control consumer does not pull")
            }
            Consumer::Write => {
                let Value::Bytes(chunk) = item else {
                    return Err(Error::type_error("write_stdout requires Bytes chunks"));
                };
                self.actions.push(Action::Discard);
                return Ok(Some(Step::Host(Request::Write(chunk.0.clone()))));
            }
            Consumer::Display => {
                item.persistent()?;
                if let Some(text) = crate::value::summary(item) {
                    self.actions.push(Action::Discard);
                    return Ok(Some(Step::Host(Request::Display(text))));
                }
            }
        }
        Ok(None)
    }
    pub fn cleaning(&self) -> bool {
        self.merges.iter().any(|merge| merge.borrow().cleaning())
    }
    pub fn callbacks(&self) -> Vec<crate::vm::callback::Handle<'gc>> {
        self.merges
            .iter()
            .flat_map(|merge| merge.borrow().callbacks())
            .collect()
    }
    pub fn producers(&self) -> Vec<Handle<'gc>> {
        self.producers
            .iter()
            .copied()
            .chain(producer::handles(self.source))
            .collect()
    }
    pub fn resources(&self) -> Vec<rill_system::resources::SourceId> {
        self.owned
            .iter()
            .copied()
            .chain(resources(self.source))
            .collect()
    }
    fn finish(&mut self, mc: &Mutation<'gc>) -> Value<'gc> {
        match &mut self.consumer {
            Consumer::One => list(mc, Vec::new()),
            Consumer::Through(_) => unreachable!("through returns its connected stream"),
            Consumer::Collect { values, .. } => list(mc, std::mem::take(values)),
            Consumer::Bytes { bytes, .. } => Value::bytes(mc, std::mem::take(bytes)),
            Consumer::Fold { accumulator, .. } => *accumulator,
            Consumer::Each(_) | Consumer::Write | Consumer::Display | Consumer::Close => {
                Value::Unit
            }
        }
    }
}

pub fn resources(root: Node<'_>) -> Vec<rill_system::resources::SourceId> {
    let mut keys = Vec::new();
    walk(root, |source| match source {
        Source::External(key) | Source::Feed { sink: key, .. } => keys.push(key),
        _ => {}
    });
    keys
}

pub(super) fn walk<'gc>(root: Node<'gc>, mut visit: impl FnMut(Source<'gc>)) {
    let mut nodes = vec![root];
    while let Some(node) = nodes.pop() {
        let Some(source) = *node.borrow() else {
            continue;
        };
        visit(source);
        match source {
            Source::Merge(merge) => nodes.extend(merge.borrow().inputs().rev()),
            Source::Zip(zip) => nodes.extend(zip.borrow().inputs.iter().rev().copied()),
            Source::Lines { input, .. }
            | Source::Map { input, .. }
            | Source::Filter { input, .. }
            | Source::Take { input, .. }
            | Source::Drop { input, .. }
            | Source::Feed { input, .. } => nodes.push(input),
            _ => {}
        }
    }
}
