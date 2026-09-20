//! Finalization is a traced continuation, so release callbacks can yield and fail safely.
use super::{Frame, Vm, descriptor, error_value, nominal};
use crate::stream::producer::{Handle, Phase, Producer, Protocol, Reason};
use crate::{
    Error,
    code::{Code, Instruction},
    host,
    modules::Key,
    value::Value,
};
use gc_arena::{Collect, Gc, Mutation, RefLock};
use rill_system::resources::SourceId;
use std::{collections::HashSet, rc::Rc};

#[derive(Clone, Copy, Collect)]
#[collect(require_static)]
pub enum After {
    Resume,
    Boundary,
    Unwind,
}

#[derive(Collect)]
#[collect(no_drop)]
enum Action<'gc> {
    Callback(super::callback::Handle<'gc>),
    CallbackDone(super::callback::Handle<'gc>),
    Producer(Handle<'gc>),
    Scope(u64),
    Invalidate(u64),
    Finish(Handle<'gc>),
    SecondCall(u64, Value<'gc>),
    CheckUnit,
    Close(#[collect(require_static)] Vec<SourceId>),
    Discard,
}
#[derive(Collect)]
#[collect(no_drop)]
pub struct Finalizer<'gc> {
    actions: Vec<Action<'gc>>,
    reply: Option<Value<'gc>>,
    raised: Option<Value<'gc>>,
    awaiting: bool,
    reason: Reason,
    after: After,
    #[collect(require_static)]
    error: Option<Error>,
}
impl<'gc> Vm<'gc> {
    pub(super) fn owner(&self) -> u64 {
        self.frames.last().map_or(0, |frame| frame.owner)
    }
    pub(super) fn producer(
        &mut self,
        mc: &Mutation<'gc>,
        protocol: Protocol<'gc>,
    ) -> Result<Value<'gc>, Error> {
        let scope =
            self.next_scope.get().checked_add(1).ok_or_else(|| {
                Error::new("LimitExceeded", "resource scope identities exhausted")
            })?;
        self.next_scope.set(scope);
        let option = self.modules()[&Key::Bundled("std:core")].field("Option")?;
        let producer = Gc::new(
            mc,
            RefLock::new(Producer::new(protocol, self.owner(), scope, option)),
        );
        self.producers.push(producer);
        Ok(crate::stream::token(
            mc,
            self.owner(),
            crate::stream::Source::Producer(producer),
        ))
    }
    pub(super) fn scoped_call(&mut self, owner: u64, function: Value<'gc>, argument: Value<'gc>) {
        let mut frame =
            self.continuation(vec![Instruction::Call { tail: false }, Instruction::Return]);
        frame.owner = owner;
        self.frames.push(frame);
        self.stack.extend([function, argument]);
    }
    pub(super) fn continuation(&self, instructions: Vec<Instruction>) -> Frame<'gc> {
        let caller = self.frames.last().expect("continuation caller");
        let span = caller.code.instructions[caller.ip - 1].1.clone();
        Frame {
            constants: caller.constants,
            code: Rc::new(Code {
                source: Rc::clone(&caller.code.source),
                context: None,
                constants: Vec::new(),
                layouts: Vec::new(),
                instructions: instructions
                    .into_iter()
                    .map(|instruction| (instruction, span.clone()))
                    .collect(),
            }),
            ip: 0,
            scopes: Vec::new(),
            stack_base: self.stack.len(),
            module: None,
            exports: crate::value::Record::new(),
            attempt: None,
            plan_base: self.plans.len(),
            resource_base: self.resources.len(),
            stream_base: self.streams.len(),
            producer_base: self.producers.len(),
            stream: None,
            work: None,
            finalizer: None,
            owner: self.owner(),
        }
    }
    pub(super) fn start_cleanup(
        &mut self,
        producers: Vec<Handle<'gc>>,
        keys: Vec<SourceId>,
        reason: Reason,
        after: After,
        error: Option<Error>,
    ) {
        let mut actions = vec![Action::Close(keys)];
        actions.extend(producers.into_iter().rev().map(Action::Producer));
        actions.extend(self.pending_callbacks.drain(..).rev().map(Action::Callback));
        if !self.cancel_operations.is_empty() {
            actions.push(Action::Close(std::mem::take(&mut self.cancel_operations)));
        }
        let mut frame = self.continuation(vec![Instruction::DriveCleanup, Instruction::Jump(0)]);
        frame.finalizer = Some(Finalizer {
            actions,
            reply: None,
            raised: self.raised.take(),
            awaiting: false,
            reason,
            after,
            error,
        });
        self.frames.push(frame);
    }
    pub(super) fn finalize_boundary(&mut self) -> bool {
        let base = self.frames.last().expect("boundary frame").producer_base;
        let producers: Vec<_> = self.producers[base..]
            .iter()
            .copied()
            .filter(|producer| active(*producer))
            .collect();
        if producers.is_empty() {
            return false;
        }
        self.frames.last_mut().expect("boundary frame").ip -= 1;
        self.start_cleanup(producers, Vec::new(), Reason::Closed, After::Boundary, None);
        true
    }
    pub(super) fn drive_cleanup(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<(), Error> {
        let mut cleanup = self
            .frames
            .last_mut()
            .expect("cleanup frame")
            .finalizer
            .take()
            .expect("finalizer");
        if cleanup.awaiting {
            cleanup.reply = Some(self.pop());
            cleanup.awaiting = false;
        }
        let result = self.cleanup_action(mc, &mut cleanup);
        self.frames.last_mut().expect("cleanup frame").finalizer = Some(cleanup);
        match result? {
            Some(crate::stream::Step::Callback(handle, invocation)) => {
                self.callback(mc, handle, invocation, fuel)?;
            }
            Some(crate::stream::Step::ScopedCall(owner, function, argument)) => {
                self.scoped_call(owner, function, argument);
            }
            Some(crate::stream::Step::Host(request)) => self.suspend(request),
            Some(crate::stream::Step::Done(_)) => return self.finish_cleanup(mc),
            None => {}
            _ => unreachable!("cleanup only calls, suspends or completes"),
        }
        Ok(())
    }
    fn cleanup_action(
        &self,
        mc: &Mutation<'gc>,
        cleanup: &mut Finalizer<'gc>,
    ) -> Result<Option<crate::stream::Step<'gc>>, Error> {
        use crate::stream::Step;
        let Some(action) = cleanup.actions.pop() else {
            return Ok(Some(Step::Done(Value::Unit)));
        };
        match action {
            Action::Callback(handle) => return Ok(cleanup.callback(mc, handle)),
            Action::CallbackDone(handle) => cleanup.callback_done(mc, handle)?,
            Action::Producer(producer) => {
                if !active(producer) {
                    return Ok(None);
                }
                let mut state = producer.borrow_mut(mc);
                let scope = state.scope;
                cleanup
                    .actions
                    .extend([Action::Finish(producer), Action::Scope(scope)]);
                if let Some((release, value)) = state.begin_close() {
                    let reason = self.close_reason(mc, &cleanup.reason)?;
                    cleanup
                        .actions
                        .extend([Action::CheckUnit, Action::SecondCall(scope, reason)]);
                    cleanup.awaiting = true;
                    return Ok(Some(Step::ScopedCall(scope, release, value)));
                }
            }
            Action::SecondCall(scope, reason) => {
                let function = cleanup.reply.take().expect("curried release");
                cleanup.awaiting = true;
                return Ok(Some(Step::ScopedCall(scope, function, reason)));
            }
            Action::CheckUnit => {
                if !matches!(cleanup.reply.take(), Some(Value::Unit)) {
                    return Err(Error::type_error("produce release must return Unit"));
                }
            }
            Action::Scope(scope) => {
                let keys = self
                    .resources
                    .iter()
                    .copied()
                    .filter(|key| self.resource_scopes.get(key) == Some(&scope))
                    .collect();
                cleanup
                    .actions
                    .extend([Action::Invalidate(scope), Action::Close(keys)]);
                cleanup.actions.extend(
                    self.producers
                        .iter()
                        .rev()
                        .copied()
                        .filter(|producer| producer.borrow().parent == scope && active(*producer))
                        .map(Action::Producer),
                );
            }
            Action::Invalidate(scope) => {
                for value in &self.streams {
                    if let Value::Stream(token) = value
                        && token.borrow().owner == scope
                    {
                        token.borrow_mut(mc).source.take();
                    }
                }
            }
            Action::Finish(producer) => producer.borrow_mut(mc).finish(),
            Action::Close(keys) if !keys.is_empty() => {
                cleanup.actions.push(Action::Discard);
                cleanup.awaiting = true;
                return Ok(Some(Step::Host(host::Request::Close(keys))));
            }
            Action::Discard => {
                cleanup.reply.take();
            }
            Action::Close(_) => {}
        }
        Ok(None)
    }
    fn close_reason(&self, mc: &Mutation<'gc>, reason: &Reason) -> Result<Value<'gc>, Error> {
        let namespace = self.modules()[&Key::Bundled("std:seq")].field("CloseReason")?;
        Ok(match reason {
            Reason::Exhausted => namespace.field("Exhausted")?,
            Reason::Closed => namespace.field("Closed")?,
            Reason::Cutoff => namespace.field("Cutoff")?,
            Reason::Cancelled => namespace.field("Cancelled")?,
            Reason::Failed(error) => nominal(
                mc,
                descriptor(namespace.field("Failed")?)?,
                [("error", error_value(mc, self.descriptors()?.error, error))],
            ),
        })
    }
    fn finish_cleanup(&mut self, mc: &Mutation<'gc>) -> Result<(), Error> {
        let frame = self.frames.pop().expect("cleanup frame");
        self.stack.truncate(frame.stack_base);
        let cleanup = frame.finalizer.expect("finalizer");
        if let Some(error) = cleanup.error {
            self.raised = cleanup.raised;
            return self.fail(mc, error);
        }
        match cleanup.after {
            After::Resume => self.stack.push(Value::Unit),
            After::Boundary => {}
            After::Unwind => unreachable!("unwind retains its error"),
        }
        Ok(())
    }
    pub(super) fn defer_error(&mut self, mc: &Mutation<'gc>, error: Error) -> bool {
        let checkpoint = self.error_checkpoint().unwrap_or(0);
        self.discard_callbacks(mc, self.callbacks(checkpoint));
        if self
            .frames
            .get(checkpoint)
            .is_some_and(|frame| frame.finalizer.is_some())
        {
            self.finished_contexts.extend(
                self.frames
                    .drain(checkpoint + 1..)
                    .filter_map(|frame| frame.code.context),
            );
            let frame = &mut self.frames[checkpoint];
            self.stack.truncate(frame.stack_base);
            frame.ip = 0;
            let cleanup = frame.finalizer.as_mut().expect("cleanup boundary");
            cleanup.awaiting = false;
            cleanup.reply = None;
            // A failed first application must not attempt to apply its missing result.
            while matches!(
                cleanup.actions.last(),
                Some(Action::SecondCall(..) | Action::CheckUnit | Action::Discard)
            ) {
                cleanup.actions.pop();
            }
            if cleanup.error.as_ref().is_some_and(Error::is_exit) {
                cleanup.error = None;
                cleanup.raised = None;
            }
            if !self.cancel_operations.is_empty() {
                cleanup
                    .actions
                    .push(Action::Close(std::mem::take(&mut self.cancel_operations)));
            }
            let raised = self.raised.take();
            if let Some(primary) = &mut cleanup.error {
                primary.notes.push(error.to_string());
                primary.notes.extend(error.notes);
            } else {
                cleanup.error = Some(error);
                cleanup.raised = raised;
            }
            return true;
        }
        let Some(frame) = self.frames.get(checkpoint) else {
            return false;
        };
        let (base, producer_base) = (frame.resource_base, frame.producer_base);
        let mut keys = Vec::new();
        let mut producers = Vec::new();
        for frame in &self.frames[checkpoint..] {
            if let Some(task) = &frame.stream {
                keys.extend(task.resources());
                producers.extend(task.producers());
            }
        }
        keys.extend(self.resources[base..].iter().copied());
        producers.extend(self.producers[producer_base..].iter().copied());
        let mut seen = HashSet::new();
        keys.retain(|key| self.resource_scopes.contains_key(key) && seen.insert(*key));
        let mut seen = HashSet::new();
        producers.retain(|producer| active(*producer) && seen.insert(Gc::as_ptr(*producer)));
        if keys.is_empty()
            && producers.is_empty()
            && self.pending_callbacks.is_empty()
            && self.cancel_operations.is_empty()
        {
            return false;
        }
        let reason = if error.is_cancelled() {
            Reason::Cancelled
        } else if error.is_exit() {
            Reason::Closed
        } else {
            Reason::Failed(error.clone())
        };
        self.start_cleanup(producers, keys, reason, After::Unwind, Some(error));
        true
    }
    pub(super) fn error_checkpoint(&self) -> Option<usize> {
        let finalizer = self
            .frames
            .iter()
            .rposition(|frame| frame.finalizer.is_some());
        self.frames
            .iter()
            .enumerate()
            .rfind(|(index, frame)| {
                frame.finalizer.is_some()
                    || (frame.attempt.is_some()
                        && (!self.aborting || finalizer.is_some_and(|base| *index > base)))
            })
            .map(|(index, _)| index)
    }
    pub fn fail(&mut self, mc: &Mutation<'gc>, error: Error) -> Result<(), Error> {
        self.aborting |= error.is_cancelled() || error.is_exit();
        if self.defer_error(mc, error.clone()) || self.catch_error(mc, &error) {
            return Ok(());
        }
        self.abort();
        Err(error)
    }
    pub fn interrupt(&mut self, mc: &Mutation<'gc>) -> Result<(), Error> {
        self.stop(mc, true)
    }
    pub fn stop(&mut self, mc: &Mutation<'gc>, cancelled: bool) -> Result<(), Error> {
        self.aborting = true;
        let error = if cancelled {
            Error::cancelled("evaluation cancelled")
        } else {
            Error::session_exit()
        };
        if let Some(cleanup) = self
            .frames
            .iter_mut()
            .find_map(|frame| frame.finalizer.as_mut())
        {
            if cleanup.error.is_none()
                || (cancelled && !cleanup.error.as_ref().is_some_and(Error::is_cancelled))
            {
                let mut error = error;
                if let Some(previous) = cleanup.error.take() {
                    error.notes.push(previous.to_string());
                    error.notes.extend(previous.notes);
                }
                cleanup.error = Some(error);
                cleanup.raised = None;
            }
            return Ok(());
        }
        let callbacks: Vec<_> = self
            .callbacks(0)
            .into_iter()
            .filter(|handle| {
                handle
                    .borrow()
                    .vm
                    .as_ref()
                    .is_some_and(|child| child.cleaning())
            })
            .collect();
        if !callbacks.is_empty() {
            for handle in callbacks {
                let mut state = handle.borrow_mut(mc);
                if let Some(child) = state.vm.as_mut()
                    && let Err(error) = child.stop(mc, cancelled)
                {
                    state.error = Some(error);
                    state.vm = None;
                }
            }
            return Ok(());
        }
        self.callback_reply = None;
        self.callback_start = None;
        self.host_request = None;
        self.host_origin = None;
        self.request = None;
        self.fail(mc, error)
    }
    pub(super) fn primary_error(&self) -> Option<Error> {
        self.frames
            .iter()
            .find_map(|frame| {
                frame
                    .finalizer
                    .as_ref()
                    .and_then(|cleanup| cleanup.error.clone())
            })
            .filter(|error| !error.is_exit() && !error.is_cancelled())
    }
    pub fn cleaning(&self) -> bool {
        self.frames.iter().any(|frame| {
            frame.finalizer.is_some() || frame.stream.as_ref().is_some_and(|task| task.cleaning())
        })
    }
}
fn active(producer: Handle<'_>) -> bool {
    !matches!(producer.borrow().phase, Phase::Closing | Phase::Closed)
}

impl<'gc> Finalizer<'gc> {
    fn callback(
        &mut self,
        mc: &Mutation<'gc>,
        handle: super::callback::Handle<'gc>,
    ) -> Option<crate::stream::Step<'gc>> {
        let mut state = handle.borrow_mut(mc);
        if !state.closing {
            state.closing = true;
            state.retained_error = state.vm.as_ref().and_then(|child| child.primary_error());
            if let Some(child) = state.vm.as_mut()
                && let Err(error) = child.stop(mc, false)
            {
                state.error = Some(error);
                state.vm = None;
            }
        }
        self.actions.push(Action::CallbackDone(handle));
        if state.vm.is_some() {
            self.awaiting = true;
            return Some(crate::stream::Step::Callback(handle, None));
        }
        None
    }
    fn callback_done(
        &mut self,
        mc: &Mutation<'gc>,
        handle: super::callback::Handle<'gc>,
    ) -> Result<(), Error> {
        self.reply.take();
        let mut state = handle.borrow_mut(mc);
        if state.vm.is_some() {
            self.actions.push(Action::Callback(handle));
        } else {
            let error = state.error.take().filter(|error| !error.is_exit());
            if let Some(mut primary) = state.retained_error.take() {
                if let Some(error) = error {
                    primary.notes.push(error.to_string());
                    primary.notes.extend(error.notes);
                }
                return Err(primary);
            }
            if let Some(error) = error {
                return Err(error);
            }
        }
        Ok(())
    }
}
