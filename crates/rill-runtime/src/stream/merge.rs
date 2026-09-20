//! Fair pull scheduling. Each branch retains one continuation, never a queue of items.
use super::{Node, Step, Task};
use crate::vm::callback::{Callback, Handle};
use crate::{Error, host::Request, operations::list, value::Value};
use gc_arena::{Collect, Gc, Mutation, RefLock};
use rill_system::resources::SourceId;

#[derive(Collect)]
#[collect(no_drop)]
struct Branch<'gc> {
    task: Task<'gc>,
    #[collect(require_static)]
    pending: Vec<SourceId>,
    ended: bool,
    callback: Option<Handle<'gc>>,
}

#[derive(Collect)]
#[collect(no_drop)]
pub struct Merge<'gc> {
    branches: Vec<Branch<'gc>>,
    cursor: usize,
    remaining: usize,
    stop_at: Option<usize>,
    #[collect(require_static)]
    reply: Option<Reply>,
}

enum Reply {
    Callback(usize),
    Forward(usize),
    Probe { branch: usize, keys: Vec<SourceId> },
    Ready(Vec<(usize, usize)>),
}

impl<'gc> Merge<'gc> {
    pub fn new(inputs: Vec<Node<'gc>>) -> Self {
        Self {
            remaining: inputs.len(),
            branches: inputs
                .into_iter()
                .map(|source| Branch {
                    task: Task::pull_one(source),
                    pending: Vec::new(),
                    ended: false,
                    callback: None,
                })
                .collect(),
            stop_at: None,
            cursor: 0,
            reply: None,
        }
    }

    pub fn through(output: Node<'gc>, feed: Node<'gc>) -> Self {
        Self {
            stop_at: Some(0),
            ..Self::new(vec![output, feed])
        }
    }

    pub fn inputs(&self) -> impl DoubleEndedIterator<Item = Node<'gc>> + '_ {
        self.branches.iter().map(|branch| branch.task.source)
    }

    pub fn cleaning(&self) -> bool {
        self.branches
            .iter()
            .filter_map(|branch| branch.callback)
            .any(|handle| {
                handle
                    .borrow()
                    .vm
                    .as_ref()
                    .is_some_and(|child| child.cleaning())
            })
    }
    pub fn callbacks(&self) -> Vec<Handle<'gc>> {
        self.branches
            .iter()
            .filter_map(|branch| branch.callback)
            .collect()
    }
    fn deliver(&mut self, mc: &Mutation<'gc>, index: usize, value: Value<'gc>) {
        let branch = &mut self.branches[index];
        if let Some(callback) = branch.callback {
            Callback::resume(callback, mc, value);
        } else {
            branch.task.resume(value);
        }
    }
    pub fn resume(&mut self, mc: &Mutation<'gc>, value: Value<'gc>) -> Result<(), Error> {
        match self.reply.take().expect("pending merge continuation") {
            Reply::Callback(index) => self.cursor = (index + 1) % self.branches.len(),
            Reply::Forward(index) => {
                self.branches[index].task.resume(value);
                self.remaining = self.remaining.saturating_add(1);
            }
            Reply::Probe { branch, keys } => {
                if matches!(value, Value::Null) {
                    self.branches[branch].pending = keys;
                    self.cursor = (branch + 1) % self.branches.len();
                } else {
                    self.branches[branch].pending.clear();
                    self.deliver(mc, branch, value);
                    self.cursor = branch;
                    self.remaining = self.remaining.saturating_add(1);
                }
            }
            Reply::Ready(routes) => {
                let (index, item) = ready(value)?;
                let &(branch, input) = routes
                    .get(index)
                    .ok_or_else(|| Error::type_error("invalid readiness index"))?;
                self.branches[branch].pending.clear();
                self.deliver(
                    mc,
                    branch,
                    list(
                        mc,
                        vec![
                            Value::Int(i64::try_from(input).map_err(|_| Error::arithmetic())?),
                            item,
                        ],
                    ),
                );
                self.cursor = branch;
                self.remaining = self.branches.len();
            }
        }
        Ok(())
    }

    pub fn resume_operation(
        &mut self,
        mc: &Mutation<'gc>,
        index: usize,
        result: Result<crate::host::Response, Error>,
    ) {
        let (branch, input) = match self.reply.take().expect("pending merge operation") {
            Reply::Probe { branch, .. } | Reply::Forward(branch) => (branch, index),
            Reply::Ready(routes) => routes[index],
            Reply::Callback(_) => unreachable!("VM owns callback replies"),
        };
        let target = &mut self.branches[branch];
        target.pending.clear();
        if let Some(handle) = target.callback {
            Callback::operation(handle, mc, input, result);
        } else if let Err(error) = target.task.resume_operation(mc, input, result) {
            target.task.error = Some(Box::new(error));
        }
        self.cursor = branch;
        self.remaining = self.branches.len();
    }
    pub fn resume_error(&mut self, mc: &Mutation<'gc>, index: usize, error: Error) {
        let (branch, input) = match self.reply.take().expect("pending merge read") {
            Reply::Probe { branch, .. } | Reply::Forward(branch) => (branch, index),
            Reply::Ready(routes) => routes[index],
            Reply::Callback(_) => unreachable!("VM owns callback replies"),
        };
        let target = &mut self.branches[branch];
        target.pending.clear();
        if let Some(handle) = target.callback {
            let mut state = handle.borrow_mut(mc);
            state.pending.clear();
            if let Err(error) = state
                .vm
                .as_mut()
                .expect("waiting callback")
                .resume_read_error(mc, input, error)
            {
                state.error = Some(error);
                state.vm = None;
            }
        } else {
            target.task.resume_read_error(mc, input, error);
        }
        self.cursor = branch;
        self.remaining = self.branches.len();
    }
    pub fn advance(&mut self, mc: &Mutation<'gc>) -> Result<Step<'gc>, Error> {
        while self.remaining != 0 {
            self.remaining -= 1;
            let index = self.cursor;
            let branch = &mut self.branches[index];
            if branch.ended {
                self.cursor = (index + 1) % self.branches.len();
                continue;
            }
            if let Some(handle) = branch.callback {
                let mut state = handle.borrow_mut(mc);
                if let Some(error) = state.error.take() {
                    return Err(error);
                }
                if let Some(value) = state.value.take() {
                    branch.pending.clear();
                    branch.callback = None;
                    branch.task.resume(value);
                } else if state.pending.is_empty() {
                    branch.pending.clear();
                    self.reply = Some(Reply::Callback(index));
                    return Ok(Step::Callback(handle, None));
                } else {
                    branch.pending.clone_from(&state.pending);
                }
            }
            if !branch.pending.is_empty() {
                let keys = branch.pending.clone();
                self.reply = Some(Reply::Probe {
                    branch: index,
                    keys: keys.clone(),
                });
                return Ok(Step::Host(Request::ReadSources { keys, wait: false }));
            }
            match branch.task.advance(mc)? {
                Step::Done(value) => {
                    let Value::List(values) = value else {
                        unreachable!("single-item consumer")
                    };
                    branch.ended = values.as_slice().is_empty();
                    if branch.ended && self.stop_at == Some(index) {
                        return Ok(Step::Done(value));
                    }
                    if !branch.ended {
                        branch.task = Task::pull_one(branch.task.source);
                        self.cursor = (index + 1) % self.branches.len();
                        self.remaining = self.branches.len();
                        return Ok(Step::Done(value));
                    }
                    self.cursor = (index + 1) % self.branches.len();
                }
                step @ (Step::Call(..) | Step::ScopedCall(..)) => {
                    // Scoped producer calls preserve their child scope; ordinary transforms
                    // inherit the enclosing consumer's scope when the VM creates the callback.
                    let (owner, function, argument) = match step {
                        Step::Call(function, argument) => (None, function, argument),
                        Step::ScopedCall(owner, function, argument) => {
                            (Some(owner), function, argument)
                        }
                        _ => unreachable!("callback step"),
                    };
                    let handle = Gc::new(mc, RefLock::new(Callback::default()));
                    branch.callback = Some(handle);
                    self.reply = Some(Reply::Callback(index));
                    return Ok(Step::Callback(handle, Some((owner, function, argument))));
                }
                Step::Host(Request::ReadSources { keys, wait: true }) => {
                    self.reply = Some(Reply::Probe {
                        branch: index,
                        keys: keys.clone(),
                    });
                    return Ok(Step::Host(Request::ReadSources { keys, wait: false }));
                }
                Step::Yield => {
                    self.cursor = (index + 1) % self.branches.len();
                    return Ok(Step::Yield);
                }
                step => {
                    self.reply = Some(Reply::Forward(index));
                    return Ok(step);
                }
            }
        }
        Ok(self.settle_pass(mc))
    }
    fn settle_pass(&mut self, mc: &Mutation<'gc>) -> Step<'gc> {
        if self.branches.iter().all(|branch| branch.ended) {
            return Step::Done(list(mc, Vec::new()));
        }
        self.remaining = self.branches.len();
        if self
            .branches
            .iter()
            .any(|branch| !branch.ended && branch.pending.is_empty())
        {
            return Step::Yield;
        }
        let mut keys = Vec::new();
        let mut routes = Vec::new();
        for offset in 0..self.branches.len() {
            let index = (self.cursor + offset) % self.branches.len();
            for (input, key) in self.branches[index].pending.iter().enumerate() {
                keys.push(*key);
                routes.push((index, input));
            }
        }
        self.reply = Some(Reply::Ready(routes));
        Step::Host(Request::ReadSources { keys, wait: true })
    }
}

pub(super) fn ready(value: Value<'_>) -> Result<(usize, Value<'_>), Error> {
    let Value::List(values) = value else {
        return Err(Error::type_error("invalid source response"));
    };
    let [Value::Int(index), item @ Value::List(_)] = values.as_slice() else {
        return Err(Error::type_error("invalid source response"));
    };
    Ok((
        usize::try_from(*index).map_err(|_| Error::type_error("invalid readiness index"))?,
        *item,
    ))
}
