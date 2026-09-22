//! Merge callbacks retain independent stacks, while language identities and cleanup stay shared.
use super::Vm;
use crate::{Error, Progress, code::Instruction, value::Value};
use gc_arena::{Collect, Gc, Mutation, RefLock};

pub type Handle<'gc> = Gc<'gc, RefLock<Callback<'gc>>>;

#[derive(Collect, Default)]
#[collect(no_drop)]
pub struct Callback<'gc> {
    pub vm: Option<Box<Vm<'gc>>>,
    pub value: Option<Value<'gc>>,
    #[collect(require_static)]
    pub error: Option<Error>,
    pub closing: bool,
    pub operation: bool,
    #[collect(require_static)]
    pub retained_error: Option<Error>,
    #[collect(require_static)]
    pub pending: Vec<rill_system::resources::SourceId>,
}

impl<'gc> Vm<'gc> {
    pub(super) fn callback(
        &mut self,
        mc: &Mutation<'gc>,
        handle: Handle<'gc>,
        invocation: Option<(Option<u64>, Value<'gc>, Value<'gc>)>,
        fuel: &mut usize,
    ) -> Result<(), Error> {
        let mut state = handle.borrow_mut(mc);
        if let Some((owner, function, argument)) = invocation {
            if self.callback_depth >= 64 {
                return Err(Error::new(
                    "LimitExceeded",
                    "nested merge callback limit exceeded",
                ));
            }
            let mut frame =
                self.continuation(vec![Instruction::Call { tail: false }, Instruction::Return]);
            frame.owner = owner.unwrap_or_else(|| self.owner());
            frame.stack_base = 0;
            frame.plan_base = 0;
            frame.resource_base = 0;
            frame.stream_base = 0;
            frame.producer_base = 0;
            state.vm = Some(Box::new(Self {
                frames: vec![frame],
                stack: vec![function, argument],
                modules: self.modules,
                next_scope: std::rc::Rc::clone(&self.next_scope),
                callback_depth: self.callback_depth + 1,
                ..Self::default()
            }));
        }
        let closing = state.closing;
        let keys = if closing {
            std::mem::take(&mut state.pending)
        } else {
            Vec::new()
        };
        let child = state.vm.as_mut().expect("live merge callback");
        if !keys.is_empty() {
            child.host_request = Some(crate::host::Request::ReadSources { keys, wait: true });
        }
        match child.step_fuel(mc, fuel) {
            Ok(Progress::Complete) => {
                let mut child = state.vm.take().expect("completed callback");
                state.value = Some(child.result);
                self.adopt(&mut child);
            }
            Ok(Progress::Waiting) => {
                if let Some(request) = child.host_request.take() {
                    match request {
                        crate::host::Request::ReadSources { keys, wait: true } if !closing => {
                            state.pending = keys;
                        }
                        request @ (crate::host::Request::Run { .. }
                        | crate::host::Request::Glob(_)
                        | crate::host::Request::ReadText { .. }
                        | crate::host::Request::Write(_)
                        | crate::host::Request::WriteError(_)
                        | crate::host::Request::Display(_)
                        | crate::host::Request::Files(_)
                        | crate::host::Request::OpenFile { .. }
                        | crate::host::Request::Start(_)
                        | crate::host::Request::Stream(_)
                        | crate::host::Request::Through(_)
                        | crate::host::Request::Control {
                            operation: crate::host::Control::Wait | crate::host::Control::Foreground,
                            ..
                        }) => {
                            self.callback_start = Some(handle);
                            self.suspend(crate::host::Request::Defer(Box::new(request)));
                            return Ok(());
                        }
                        request => {
                            self.callback_reply = Some(handle);
                            self.suspend(request);
                            return Ok(());
                        }
                    }
                }
            }
            Ok(Progress::Yielded) => {}
            Err(error) => {
                state.error = Some(error);
                state.vm = None;
            }
        }
        self.stack.push(Value::Unit);
        Ok(())
    }

    pub(super) fn resume_operation(
        &mut self,
        mc: &Mutation<'gc>,
        index: usize,
        result: Result<crate::host::Response, Error>,
    ) -> Result<(), Error> {
        if let Some(handle) = self.callback_reply.take() {
            Callback::operation(handle, mc, index, result);
            return self.resume_value(mc, Ok(Value::Unit));
        }
        let task = self
            .frames
            .last_mut()
            .and_then(|frame| frame.stream.as_mut())
            .ok_or_else(|| Error::new("RuntimeError", "no callback operation is suspended"))?;
        task.resume_operation(mc, index, result)?;
        self.host_origin = None;
        self.host_request = None;
        Ok(())
    }

    pub(super) fn adopt(&mut self, child: &mut Self) {
        self.resources.append(&mut child.resources);
        self.resource_scopes.extend(child.resource_scopes.drain());
        self.streams.append(&mut child.streams);
        self.producers.append(&mut child.producers);
        self.finished_contexts.append(&mut child.finished_contexts);
    }

    pub(super) fn discard_callbacks(&mut self, mc: &Mutation<'gc>, handles: Vec<Handle<'gc>>) {
        let mut pending: Vec<_> = handles.into_iter().rev().collect();
        while let Some(handle) = pending.pop() {
            let mut state = handle.borrow_mut(mc);
            if state.vm.as_ref().is_some_and(|child| child.cleaning()) {
                self.pending_callbacks.push(handle);
            } else if let Some(mut child) = state.vm.take() {
                if state.operation {
                    self.cancel_operations.append(&mut state.pending);
                    state.operation = false;
                }
                pending.extend(child.callbacks(0).into_iter().rev());
                self.adopt(&mut child);
            }
        }
    }

    pub(super) fn callbacks(&self, from: usize) -> Vec<Handle<'gc>> {
        self.frames[from..]
            .iter()
            .filter_map(|frame| frame.stream.as_ref())
            .flat_map(|task| task.callbacks())
            .collect()
    }
}

impl Callback<'_> {
    pub fn started<'gc>(
        handle: Handle<'gc>,
        mc: &Mutation<'gc>,
        response: Result<crate::host::Response, Error>,
    ) {
        let mut state = handle.borrow_mut(mc);
        let child = state.vm.as_mut().expect("starting callback operation");
        if let Ok(crate::host::Response::Deferred(key)) = response {
            child.resources.push(key);
            child.resource_scopes.insert(key, child.owner());
            state.pending = vec![key];
            state.operation = true;
        } else if let Err(error) = child.resume(mc, response) {
            state.error = Some(error);
            state.vm = None;
        }
    }
    pub fn operation<'gc>(
        handle: Handle<'gc>,
        mc: &Mutation<'gc>,
        index: usize,
        result: Result<crate::host::Response, Error>,
    ) {
        let mut state = handle.borrow_mut(mc);
        let keys = std::mem::take(&mut state.pending);
        let direct = std::mem::take(&mut state.operation);
        let child = state.vm.as_mut().expect("waiting callback operation");
        let result = if direct {
            for key in keys {
                child.resources.retain(|entry| *entry != key);
                child.resource_scopes.remove(&key);
            }
            child.resume(mc, result)
        } else {
            child.resume_operation(mc, index, result)
        };
        if let Err(error) = result {
            state.error = Some(error);
            state.vm = None;
        }
    }

    pub fn resume<'gc>(handle: Handle<'gc>, mc: &Mutation<'gc>, value: Value<'gc>) {
        let mut state = handle.borrow_mut(mc);
        state.pending.clear();
        if let Err(error) = state
            .vm
            .as_mut()
            .expect("waiting callback")
            .resume_value(mc, Ok(value))
        {
            state.error = Some(error);
            state.vm = None;
        }
    }
}
