//! Demand-driven producer state. VM continuations own callbacks and explicit finalization.
use crate::{Error, value::Value};
use gc_arena::{Collect, Gc, RefLock};

pub type Handle<'gc> = Gc<'gc, RefLock<Producer<'gc>>>;

#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
pub struct Protocol<'gc> {
    pub acquire: Value<'gc>,
    pub step: Value<'gc>,
    pub release: Value<'gc>,
}
impl<'gc> Protocol<'gc> {
    pub fn new(value: Value<'gc>) -> Result<Self, Error> {
        let Value::Record(fields) = value else {
            return Err(Error::type_error(
                "produce requires {acquire, step, release}",
            ));
        };
        if fields.len() != 3 {
            return Err(Error::type_error(
                "produce requires exactly acquire, step and release",
            ));
        }
        let protocol = Self {
            acquire: value.field("acquire")?,
            step: value.field("step")?,
            release: value.field("release")?,
        };
        for callback in [protocol.acquire, protocol.step, protocol.release] {
            super::callable(callback)?;
        }
        Ok(protocol)
    }
}
#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
pub enum Phase<'gc> {
    New,
    Acquiring,
    Ready(Value<'gc>),
    Stepping(Value<'gc>),
    Closing,
    Closed,
}
#[derive(Clone, Copy, Collect)]
#[collect(require_static)]
pub enum Callback {
    Acquire,
    Step,
}
#[derive(Clone, Collect)]
#[collect(require_static)]
pub enum Reason {
    Exhausted,
    Closed,
    Cutoff,
    Failed(Error),
    Cancelled,
}
#[derive(Collect)]
#[collect(no_drop)]
pub struct Producer<'gc> {
    pub parent: u64,
    pub scope: u64,
    pub phase: Phase<'gc>,
    pub protocol: Protocol<'gc>,
    option: Value<'gc>,
}
impl<'gc> Producer<'gc> {
    pub const fn new(protocol: Protocol<'gc>, parent: u64, scope: u64, option: Value<'gc>) -> Self {
        Self {
            parent,
            scope,
            phase: Phase::New,
            protocol,
            option,
        }
    }
    pub fn pull(&mut self) -> Result<Option<(Callback, Value<'gc>, Value<'gc>)>, Error> {
        match self.phase {
            Phase::New => {
                self.phase = Phase::Acquiring;
                Ok(Some((
                    Callback::Acquire,
                    self.protocol.acquire,
                    Value::Unit,
                )))
            }
            Phase::Ready(state) => {
                self.phase = Phase::Stepping(state);
                Ok(Some((Callback::Step, self.protocol.step, state)))
            }
            Phase::Closed => Ok(None),
            Phase::Acquiring | Phase::Stepping(_) | Phase::Closing => Err(Error::new(
                "StreamBusy",
                "producer is already active or closing",
            )),
        }
    }
    pub fn acquired(&mut self, state: Value<'gc>) -> Result<(), Error> {
        state.within_scope(Some(self.scope))?;
        self.phase = Phase::Ready(state);
        Ok(())
    }
    pub fn stepped(&mut self, result: Value<'gc>) -> Result<Option<Value<'gc>>, Error> {
        let Phase::Stepping(previous) = self.phase else {
            unreachable!("producer step")
        };
        let Value::Adt(value) = result else {
            return Err(Error::type_error(
                "produce step requires Option of [item, next_state]",
            ));
        };
        let Value::Adt(none) = self.option.field("None")? else {
            unreachable!("Option.None")
        };
        let Value::Constructor(some) = self.option.field("Some")? else {
            unreachable!("Option.Some")
        };
        if Gc::ptr_eq(value.descriptor, none.descriptor) {
            self.phase = Phase::Ready(previous);
            return Ok(None);
        }
        if !Gc::ptr_eq(value.descriptor, some) {
            return Err(Error::type_error(
                "produce step requires the shared Option type",
            ));
        }
        let Value::List(pair) = result.field("value")? else {
            return Err(Error::type_error(
                "produce step requires [item, next_state]",
            ));
        };
        let [item, next] = pair.as_slice() else {
            return Err(Error::type_error(
                "produce step requires [item, next_state]",
            ));
        };
        item.persistent()?;
        next.within_scope(Some(self.scope))?;
        self.phase = Phase::Ready(*next);
        Ok(Some(*item))
    }
    pub const fn begin_close(&mut self) -> Option<(Value<'gc>, Value<'gc>)> {
        let previous = std::mem::replace(&mut self.phase, Phase::Closing);
        match previous {
            Phase::Ready(state) | Phase::Stepping(state) => Some((self.protocol.release, state)),
            Phase::Closed => {
                self.phase = Phase::Closed;
                None
            }
            Phase::New | Phase::Acquiring | Phase::Closing => None,
        }
    }
    pub const fn finish(&mut self) {
        self.phase = Phase::Closed;
        self.protocol = Protocol {
            acquire: Value::Unit,
            step: Value::Unit,
            release: Value::Unit,
        };
        self.option = Value::Unit;
    }
}

pub fn handles(root: super::Node<'_>) -> Vec<Handle<'_>> {
    let mut result = Vec::new();
    super::machine::walk(root, |source| {
        if let super::Source::Producer(producer) = source {
            result.push(producer);
        }
    });
    result
}
