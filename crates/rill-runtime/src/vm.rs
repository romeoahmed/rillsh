//! A resumable stack machine. A quantum ends before collection or host scheduling.
pub mod callback;
mod cleanup;
use crate::{
    Error, Progress,
    code::{Code, FunctionCode, Instruction, Reference},
    host,
    modules::{Key, Request},
    native::{self, Intrinsic},
    operations, pattern,
    scope::Scope,
    value::{Adt, Closure, Descriptor, Value},
};
use gc_arena::{Collect, Gc, Mutation, RefLock};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Collect)]
#[collect(no_drop)]
struct Frame<'gc> {
    #[collect(require_static)]
    code: Rc<Code>,
    constants: crate::code::Constants<'gc>,
    ip: usize,
    scopes: Vec<Scope<'gc>>,
    stack_base: usize,
    #[collect(require_static)]
    module: Option<(Key, String)>,
    exports: crate::value::Record<'gc>,
    attempt: Option<Attempt<'gc>>,
    plan_base: usize,
    resource_base: usize,
    stream_base: usize,
    stream: Option<Box<crate::stream::Task<'gc>>>,
    work: Option<Box<crate::native::work::Work<'gc>>>,
    finalizer: Option<cleanup::Finalizer<'gc>>,
    owner: u64,
    producer_base: usize,
}

#[derive(Clone, Copy, Collect)]
#[collect(no_drop)]
struct Attempt<'gc> {
    ok: Gc<'gc, Descriptor>,
    err: Gc<'gc, Descriptor>,
    error: Gc<'gc, Descriptor>,
}

#[derive(Collect)]
#[collect(no_drop)]
pub struct Vm<'gc> {
    pub globals: HashMap<String, Value<'gc>>,
    frames: Vec<Frame<'gc>>,
    stack: Vec<Value<'gc>>,
    pub result: Value<'gc>,
    pub display: bool,
    raised: Option<Value<'gc>>,
    #[collect(require_static)]
    resources: Vec<rill_system::resources::SourceId>,
    #[collect(require_static)]
    resource_scopes: HashMap<rill_system::resources::SourceId, u64>,
    producers: Vec<crate::stream::producer::Handle<'gc>>,
    #[collect(require_static)]
    next_scope: Rc<std::cell::Cell<u64>>,
    callback_depth: usize,
    callback_reply: Option<callback::Handle<'gc>>,
    callback_start: Option<callback::Handle<'gc>>,
    pending_callbacks: Vec<callback::Handle<'gc>>,
    #[collect(require_static)]
    cancel_operations: Vec<rill_system::resources::SourceId>,
    aborting: bool,
    streams: Vec<Value<'gc>>,
    #[collect(require_static)]
    plans: Vec<rill_system::plan::Plan>,
    nominal_names: HashSet<String>,
    pending_nominals: HashSet<String>,
    modules: Option<Gc<'gc, RefLock<HashMap<Key, Value<'gc>>>>>,
    #[collect(require_static)]
    pub request: Option<Request>,
    #[collect(require_static)]
    pub host_request: Option<host::Request>,
    #[collect(require_static)]
    host_origin: Option<(Rc<crate::Source>, rill_syntax::token::Span)>,
    pub finished_contexts: Vec<usize>,
    pub published_modules: Vec<Key>,
}

impl Default for Vm<'_> {
    fn default() -> Self {
        Self {
            globals: HashMap::new(),
            frames: Vec::new(),
            stack: Vec::new(),
            result: Value::Unit,
            display: false,
            raised: None,
            resources: Vec::new(),
            resource_scopes: HashMap::new(),
            producers: Vec::new(),
            next_scope: Rc::new(std::cell::Cell::new(0)),
            callback_depth: 0,
            callback_reply: None,
            callback_start: None,
            pending_callbacks: Vec::new(),
            cancel_operations: Vec::new(),
            aborting: false,
            streams: Vec::new(),
            plans: Vec::new(),
            nominal_names: HashSet::new(),
            pending_nominals: HashSet::new(),
            modules: None,
            request: None,
            host_request: None,
            host_origin: None,
            finished_contexts: Vec::new(),
            published_modules: Vec::new(),
        }
    }
}

impl<'gc> Vm<'gc> {
    /// Move session-wide identities without retaining stale namespaces in parked contexts.
    pub fn transfer_session(&mut self, target: &mut Self) {
        target.globals = std::mem::take(&mut self.globals);
        target.modules = self.modules.take();
        target.nominal_names = std::mem::take(&mut self.nominal_names);
        target.next_scope = Rc::clone(&self.next_scope);
    }

    fn modules(&self) -> std::cell::Ref<'_, HashMap<Key, Value<'gc>>> {
        self.modules
            .as_ref()
            .expect("initialized module cache")
            .borrow()
    }
    pub fn install_prelude(&mut self) {
        let Value::Record(exports) = self.modules()[&Key::Bundled("std:prelude")] else {
            unreachable!("module namespace")
        };
        self.globals = exports
            .iter()
            .map(|(name, value)| (name.clone(), *value))
            .collect();
    }
    pub const fn active(&self) -> bool {
        !self.frames.is_empty()
    }
    pub fn begin(&mut self, mc: &Mutation<'gc>, code: Rc<Code>) {
        self.abort();
        self.modules
            .get_or_insert_with(|| Gc::new(mc, RefLock::new(HashMap::new())));
        self.frames.push(Frame {
            constants: code.cache(mc),
            scopes: vec![Scope::with_snapshot(
                mc,
                &code.layouts[0],
                self.globals.clone(),
            )],
            code,
            ip: 0,
            stack_base: 0,
            module: None,
            exports: crate::value::Record::new(),
            attempt: None,
            plan_base: self.plans.len(),
            resource_base: self.resources.len(),
            stream_base: self.streams.len(),
            stream: None,
            work: None,
            finalizer: None,
            owner: self.owner(),
            producer_base: self.producers.len(),
        });
    }
    pub fn abort(&mut self) {
        self.finished_contexts
            .extend(self.frames.iter().filter_map(|frame| frame.code.context));
        self.frames.clear();
        self.callback_reply = None;
        self.callback_start = None;
        self.pending_callbacks.clear();
        self.cancel_operations.clear();
        self.request = None;
        self.host_request = None;
        self.host_origin = None;
        self.raised = None;
        self.streams.clear();
        self.resources.clear();
        self.resource_scopes.clear();
        self.producers.clear();
        self.aborting = false;
        self.plans.clear();
        self.stack.clear();
        self.result = Value::Unit;
        self.display = false;
        self.pending_nominals.clear();
    }
    pub fn importing(&self, key: &Key) -> bool {
        self.frames.iter().any(|frame| {
            frame
                .module
                .as_ref()
                .is_some_and(|(active, _)| active == key)
        })
    }
    pub fn imported(&mut self, key: &Key, name: &str) -> Result<bool, Error> {
        let namespace = self.modules().get(key).copied();
        if let Some(namespace) = namespace {
            self.scope().insert(name, namespace);
            return Ok(true);
        }
        if self.importing(key) {
            return Err(Error::new(
                "ImportCycle",
                "module imports itself through an active dependency",
            ));
        }
        if self
            .frames
            .iter()
            .filter(|frame| frame.module.is_some())
            .count()
            >= 256
        {
            return Err(Error::new(
                "LimitExceeded",
                "active module import limit exceeded",
            ));
        }
        Ok(false)
    }
    pub fn begin_module(&mut self, mc: &Mutation<'gc>, key: Key, name: String, code: Rc<Code>) {
        self.frames.push(Frame {
            constants: code.cache(mc),
            ip: 0,
            scopes: vec![Scope::with_snapshot(
                mc,
                &code.layouts[0],
                if matches!(key, Key::Bundled(_)) {
                    native::bindings()
                } else {
                    self.modules()
                        .get(&Key::Bundled("std:prelude"))
                        .map_or_else(HashMap::new, |namespace| {
                            let Value::Record(fields) = namespace else {
                                unreachable!("module namespace")
                            };
                            fields
                                .iter()
                                .map(|(name, value)| (name.clone(), *value))
                                .collect()
                        })
                },
            )],
            code,
            stack_base: self.stack.len(),
            module: Some((key, name)),
            exports: crate::value::Record::new(),
            attempt: None,
            plan_base: self.plans.len(),
            resource_base: self.resources.len(),
            stream_base: self.streams.len(),
            stream: None,
            work: None,
            finalizer: None,
            owner: self.owner(),
            producer_base: self.producers.len(),
        });
    }
    fn pop(&mut self) -> Value<'gc> {
        self.stack
            .pop()
            .expect("compiler balances the operand stack")
    }
    fn lookup(&self, name: &str) -> Result<Value<'gc>, Error> {
        self.frames
            .last()
            .and_then(|frame| frame.scopes.iter().rev().find_map(|scope| scope.get(name)))
            .copied()
            .ok_or_else(|| Error::new("NameError", format!("unknown binding '{name}'")))
    }
    fn load(&self, reference: &Reference) -> Result<Value<'gc>, Error> {
        match reference {
            Reference::Global(name) => self.lookup(name),
            Reference::Local { scope, slot } => {
                let frame = self.frames.last().expect("active reference frame");
                Ok(frame.scopes[*scope].slots[*slot].expect("initialized lexical slot"))
            }
        }
    }
    fn scope(&mut self) -> &mut Scope<'gc> {
        self.frames
            .last_mut()
            .expect("active frame")
            .scopes
            .last_mut()
            .expect("active lexical scope")
    }
    pub fn step(&mut self, mc: &Mutation<'gc>, mut fuel: usize) -> Result<Progress, Error> {
        self.step_fuel(mc, &mut fuel)
    }
    fn step_fuel(&mut self, mc: &Mutation<'gc>, fuel: &mut usize) -> Result<Progress, Error> {
        if self.host_origin.is_some() {
            return Ok(Progress::Waiting);
        }
        while *fuel != 0 {
            let Some(frame) = self.frames.last_mut() else {
                return Ok(Progress::Complete);
            };
            // Hold executable storage across calls that may replace the active frame.
            let code = Rc::clone(&frame.code);
            let (result, span) = if let Some(work) = &mut frame.work {
                let span = frame.code.instructions[frame.ip - 1].1.clone();
                let result = work.advance(mc, fuel).map(|result| {
                    if let Some(value) = result {
                        frame.work = None;
                        self.stack.push(value);
                    }
                });
                (result, span)
            } else {
                *fuel -= 1;
                let (instruction, span) = &code.instructions[frame.ip];
                frame.ip += 1;
                (self.instruction(mc, instruction, fuel), span.clone())
            };
            if let Err(mut error) = result {
                if error.origin.is_none() {
                    error.span = Some(span);
                    error.origin = Some(Rc::clone(&code.source));
                }
                self.fail(mc, error)?;
                return Ok(Progress::Yielded);
            }
            if self.request.is_some() || self.host_origin.is_some() {
                return Ok(Progress::Waiting);
            }
            if crate::heap::should_yield(mc, *fuel) {
                return Ok(Progress::Yielded);
            }
        }
        Ok(if self.frames.is_empty() {
            Progress::Complete
        } else {
            Progress::Yielded
        })
    }
    fn instruction(
        &mut self,
        mc: &Mutation<'gc>,
        instruction: &Instruction,
        fuel: &mut usize,
    ) -> Result<(), Error> {
        match instruction {
            Instruction::DriveStream => self.drive_stream(mc, fuel)?,
            Instruction::DriveCleanup => self.drive_cleanup(mc, fuel)?,
            Instruction::Boundary { last } => self.boundary(mc, *last)?,
            Instruction::BeginPlan
            | Instruction::BeginStage
            | Instruction::Argument { .. }
            | Instruction::Redirect(_)
            | Instruction::EndPlan => self.plan_instruction(mc, instruction)?,
            Instruction::RunPlan => {
                let argument = self.pop();
                self.native(mc, Intrinsic::Host("run"), argument, false)?;
            }
            Instruction::Import { path, name } => self.request_import(path.clone(), name.clone()),
            Instruction::Export(fields) => self.export(fields)?,
            Instruction::Constant(index) => self.constant(mc, *index)?,
            Instruction::Load(reference) => self.stack.push(self.load(reference)?),
            Instruction::Pop => {
                self.pop();
            }
            Instruction::List(count) => {
                let values = self.stack.split_off(self.stack.len() - *count);
                self.stack.push(operations::list(mc, values));
            }
            Instruction::Record(keys) => self.record(mc, keys),
            Instruction::Field(name) => {
                let base = self.pop();
                self.stack.push(base.field(name)?);
            }
            Instruction::Index => {
                let index = self.pop();
                let base = self.pop();
                self.stack.push(operations::index(base, index)?);
            }
            Instruction::Unary(op) => {
                let value = self.pop();
                self.stack.push(operations::unary(*op, value)?);
            }
            Instruction::Binary(op) => self.binary(mc, *op)?,
            Instruction::Enter(layout) => {
                let frame = self.frames.last_mut().expect("active frame");
                frame.scopes.push(Scope::new(&frame.code.layouts[*layout]));
            }
            Instruction::Leave => {
                self.frames.last_mut().expect("active frame").scopes.pop();
            }
            Instruction::Bind(pattern) => {
                let value = self.pop();
                let names = pattern::bind(mc, pattern, value, &|name| self.lookup(name))?;
                self.scope().extend(names);
            }
            Instruction::Closure(code) => self.closure(mc, Rc::clone(code))?,
            Instruction::Functions(codes) => self.functions(mc, codes)?,
            Instruction::Struct(name, fields) => {
                self.reserve_nominal(name)?;
                let value = constructor(mc, name.clone(), fields.clone());
                self.scope().insert(name, value);
            }
            Instruction::Enum(name, cases) => self.declare_enum(mc, name, cases)?,
            Instruction::TryBind(pattern, failure) => {
                let subject = *self.stack.last().expect("match subject");
                if let Some(bindings) =
                    pattern::attempt(mc, pattern, subject, &|name| self.lookup(name))?
                {
                    self.scope().extend(bindings);
                } else {
                    self.jump(*failure);
                }
            }
            Instruction::NoMatch => {
                return Err(Error::new("MatchError", "no match arm accepted the value"));
            }
            Instruction::Call { tail } => self.call(mc, *tail)?,
            Instruction::Branch(target) => {
                if !boolean(self.pop())? {
                    self.jump(*target);
                }
            }
            Instruction::Jump(target) => self.jump(*target),
            Instruction::Bool => {
                boolean(*self.stack.last().expect("boolean operand"))?;
            }
            Instruction::Swap => {
                let len = self.stack.len();
                self.stack.swap(len - 1, len - 2);
            }
            Instruction::Return => self.return_value(mc)?,
        }
        Ok(())
    }
    fn binary(&mut self, mc: &Mutation<'gc>, op: rill_syntax::ast::Binary) -> Result<(), Error> {
        let right = self.pop();
        let left = self.pop();
        if matches!(
            op,
            rill_syntax::ast::Binary::Equal | rill_syntax::ast::Binary::NotEqual
        ) {
            let negate = op == rill_syntax::ast::Binary::NotEqual;
            if let Some(equal) = crate::equality::immediate(left, right)? {
                self.stack.push(Value::Bool(equal != negate));
            } else {
                self.frames.last_mut().expect("comparison frame").work =
                    Some(Box::new(native::work::Work::Equality(
                        crate::equality::Equality::new(left, right, negate),
                    )));
            }
        } else {
            self.stack.push(operations::binary(mc, op, left, right)?);
        }
        Ok(())
    }
    fn constant(&mut self, mc: &Mutation<'gc>, index: usize) -> Result<(), Error> {
        let frame = self.frames.last().expect("constant loader");
        let cached = frame.constants.borrow()[index];
        let value = if let Some(value) = cached {
            value
        } else {
            let value = operations::literal(mc, &frame.code.constants[index])?;
            frame.constants.borrow_mut(mc)[index] = Some(value);
            value
        };
        self.stack.push(value);
        Ok(())
    }
    fn record(&mut self, mc: &Mutation<'gc>, keys: &[String]) {
        let values = self.stack.split_off(self.stack.len() - keys.len());
        self.stack.push(Value::Record(crate::heap::record(
            mc,
            keys.iter().cloned().zip(values).collect(),
        )));
    }
    fn boundary(&mut self, mc: &Mutation<'gc>, last: bool) -> Result<(), Error> {
        if last
            && self.display
            && self.frames.len() == 1
            && matches!(self.stack.last(), Some(Value::Stream(_)))
        {
            let value = self.pop();
            let task = crate::stream::display(mc, value, self.owner())?;
            // Retry publication only after display and upstream cleanup have completed.
            self.frames.last_mut().expect("entry frame").ip -= 1;
            self.start_stream(Box::new(task), false);
            return Ok(());
        }
        if self.finalize_boundary() {
            self.frames.last_mut().expect("cleanup frame").ip = 0;
            return Ok(());
        }
        let frame = self.frames.last().expect("statement frame");
        let (stream_base, resource_base) = (frame.stream_base, frame.resource_base);
        if self.streams.len() > stream_base {
            let result = *self.stack.last().expect("statement result");
            if matches!(result, Value::Stream(_)) {
                return Err(Error::new(
                    "UnconsumedStream",
                    "consume a stream before returning it from a statement",
                ));
            }
            result.persistent()?;
            let frame = self.frames.last().expect("statement frame");
            for value in frame
                .scopes
                .iter()
                .flat_map(Scope::values)
                .chain(frame.exports.values())
            {
                value.persistent()?;
            }
            for stream in self.streams.drain(stream_base..) {
                if let Value::Stream(stream) = stream {
                    stream.borrow_mut(mc).source.take();
                }
            }
        }
        if self.resources.len() > resource_base {
            let keys = self.resources.drain(resource_base..).collect();
            self.start_cleanup(
                Vec::new(),
                keys,
                crate::stream::producer::Reason::Closed,
                cleanup::After::Boundary,
                None,
            );
        }
        Ok(())
    }
    fn plan_instruction(
        &mut self,
        mc: &Mutation<'gc>,
        instruction: &Instruction,
    ) -> Result<(), Error> {
        use rill_system::plan::{Plan, Stage};
        match instruction {
            Instruction::BeginPlan => self.plans.push(Plan { stages: Vec::new() }),
            Instruction::BeginStage => self
                .plans
                .last_mut()
                .expect("active plan")
                .stages
                .push(Stage::default()),
            Instruction::Argument { spread } => {
                let value = self.pop();
                let plan = self.plans.last_mut().expect("active plan");
                let index = plan.stages.len() - 1;
                let stage = plan.stages.last_mut().expect("active stage");
                if *spread {
                    for value in native::items(value)?.as_slice() {
                        native::plan::argument(stage, *value, index)?;
                    }
                } else {
                    native::plan::argument(stage, value, index)?;
                }
            }
            Instruction::Redirect(kind) => {
                let value = if *kind == rill_syntax::token::Redirect::ErrorToOutput {
                    None
                } else {
                    Some(self.pop())
                };
                let stage = self
                    .plans
                    .last_mut()
                    .expect("active plan")
                    .stages
                    .last_mut()
                    .expect("active stage");
                stage.redirects.push(native::plan::redirect(*kind, value)?);
            }
            Instruction::EndPlan => {
                let plan = self.plans.pop().expect("completed plan");
                self.stack
                    .push(Value::Plan(Gc::new(mc, crate::value::JobPlan(plan))));
            }
            _ => unreachable!("plan instruction"),
        }
        Ok(())
    }
    fn declare_enum(
        &mut self,
        mc: &Mutation<'gc>,
        name: &str,
        cases: &[(String, Vec<String>)],
    ) -> Result<(), Error> {
        self.reserve_nominal(name)?;
        let fields = cases
            .iter()
            .map(|(case, fields)| {
                let value = constructor(mc, format!("{name}.{case}"), fields.clone());
                (case.clone(), value)
            })
            .collect();
        self.scope()
            .insert(name, Value::Record(crate::heap::record(mc, fields)));
        Ok(())
    }
    fn request_import(&mut self, path: String, name: String) {
        let frame = self.frames.last().expect("active import frame");
        self.request = Some(Request {
            path,
            name,
            context: frame.code.context,
            source: Rc::clone(&frame.code.source),
            span: frame.code.instructions[frame.ip - 1].1.clone(),
        });
    }
    fn export(&mut self, fields: &[(String, Vec<String>)]) -> Result<(), Error> {
        let exports = fields
            .iter()
            .map(|(name, path)| {
                let value = path[1..]
                    .iter()
                    .try_fold(self.lookup(&path[0])?, |value, field| value.field(field))?;
                Ok((name.clone(), value))
            })
            .collect::<Result<_, Error>>()?;
        self.frames.last_mut().expect("active export frame").exports = exports;
        Ok(())
    }
    fn return_value(&mut self, mc: &Mutation<'gc>) -> Result<(), Error> {
        if self.frames.len() == 1
            && let Some(name) = self
                .pending_nominals
                .intersection(&self.nominal_names)
                .min()
        {
            return Err(Error::new(
                "NameError",
                format!("nominal name '{name}' was declared while this evaluation was suspended"),
            ));
        }
        let mut result = self.pop();
        let frame = self.frames.pop().expect("active frame");
        if let Some(attempt) = frame.attempt {
            result = nominal(mc, attempt.ok, [("value", result)]);
        }
        self.stack.truncate(frame.stack_base);
        self.finished_contexts.extend(frame.code.context);
        if let Some((key, name)) = frame.module {
            let namespace = Value::Record(crate::heap::record(mc, frame.exports));
            self.published_modules.push(key.clone());
            self.modules
                .as_ref()
                .expect("module cache")
                .borrow_mut(mc)
                .insert(key, namespace);
            self.scope().insert(&name, namespace);
        } else if self.frames.is_empty() {
            if let Some(scope) = frame.scopes.into_iter().next() {
                scope.publish(&mut self.globals);
            }
            self.result = result;
            self.nominal_names.extend(self.pending_nominals.drain());
        } else {
            self.stack.push(result);
        }
        Ok(())
    }
    fn reserve_nominal(&mut self, name: &str) -> Result<(), Error> {
        if self
            .frames
            .last()
            .is_some_and(|frame| frame.module.is_some())
        {
            return Ok(());
        }
        if self.nominal_names.contains(name) || !self.pending_nominals.insert(name.into()) {
            return Err(Error::new(
                "NameError",
                format!("nominal name '{name}' is already declared"),
            ));
        }
        Ok(())
    }
    fn functions(&mut self, mc: &Mutation<'gc>, codes: &[Rc<FunctionCode>]) -> Result<(), Error> {
        let names: HashSet<_> = codes.iter().filter_map(|code| code.name.clone()).collect();
        let mut functions = HashMap::new();
        for code in codes {
            let mut environment = Scope::new(&code.body.layouts[0]);
            for capture in &code.captures {
                if !names.contains(&capture.name) {
                    environment.slots[capture.slot] = Some(self.load(&capture.source)?);
                }
            }
            let name = code.name.clone().expect("named function declaration");
            functions.insert(
                name,
                Value::Function(Gc::new(
                    mc,
                    Closure {
                        constants: code.body.cache(mc),
                        code: Rc::clone(code),
                        parameter: 0,
                        environment: Gc::new(mc, RefLock::new(environment)),
                    },
                )),
            );
        }
        for value in functions.values() {
            let Value::Function(closure) = value else {
                unreachable!("function group")
            };
            let mut environment = closure.environment.borrow_mut(mc);
            for capture in &closure.code.captures {
                if let Some(value) = functions.get(&capture.name) {
                    environment.slots[capture.slot] = Some(*value);
                }
            }
            if let Some(name) = &closure.code.name {
                environment.insert(name, *value);
            }
        }
        self.scope().extend(functions);
        Ok(())
    }
    fn jump(&mut self, target: usize) {
        self.frames.last_mut().expect("active frame").ip = target;
    }
    fn closure(&mut self, mc: &Mutation<'gc>, code: Rc<FunctionCode>) -> Result<(), Error> {
        let mut captures = Scope::new(&code.body.layouts[0]);
        for capture in &code.captures {
            captures.slots[capture.slot] = Some(self.load(&capture.source)?);
        }
        let environment = Gc::new(mc, RefLock::new(captures));
        let name = code.name.clone();
        let function = Value::Function(Gc::new(
            mc,
            Closure {
                constants: code.body.cache(mc),
                code,
                parameter: 0,
                environment,
            },
        ));
        if let Some(name) = name {
            environment.borrow_mut(mc).insert(&name, function);
        }
        self.stack.push(function);
        Ok(())
    }
    fn descriptors(&self) -> Result<Attempt<'gc>, Error> {
        let core = self
            .modules()
            .get(&Key::Bundled("std:core"))
            .copied()
            .ok_or_else(|| Error::new("ImportError", "std:core must be initialized"))?;
        Ok(Attempt {
            ok: descriptor(core.field("Result")?.field("Ok")?)?,
            err: descriptor(core.field("Result")?.field("Err")?)?,
            error: descriptor(core.field("Error")?)?,
        })
    }
    fn suspend(&mut self, request: host::Request) {
        if let host::Request::Close(keys) = &request {
            let keys: HashSet<_> = keys.iter().copied().collect();
            let removed: Vec<_> = self
                .resources
                .iter()
                .enumerate()
                .filter_map(|(index, key)| keys.contains(key).then_some(index))
                .collect();
            self.resources.retain(|key| !keys.contains(key));
            self.resource_scopes.retain(|key, _| !keys.contains(key));
            for frame in &mut self.frames {
                frame.resource_base -=
                    removed.partition_point(|index| *index < frame.resource_base);
            }
        }
        let frame = self.frames.last().expect("native caller");
        self.host_origin = Some((
            Rc::clone(&frame.code.source),
            frame.code.instructions[frame.ip - 1].1.clone(),
        ));
        self.host_request = Some(request);
    }
    pub fn resume(
        &mut self,
        mc: &Mutation<'gc>,
        response: Result<host::Response, Error>,
    ) -> Result<(), Error> {
        if let Some(handle) = self.callback_start.take() {
            callback::Callback::started(handle, mc, response);
            return self.resume_value(mc, Ok(Value::Unit));
        }
        if let Ok(host::Response::Operation { index, result }) = response {
            return self.resume_operation(
                mc,
                index,
                result.map(|response| *response).map_err(Error::from),
            );
        }
        if let Some(handle) = self.callback_reply.take() {
            let mut state = handle.borrow_mut(mc);
            if let Some(child) = state.vm.as_mut()
                && let Err(error) = child.resume(mc, response)
            {
                state.error = Some(error);
                state.vm = None;
            }
            return self.resume_value(mc, Ok(Value::Unit));
        }
        if let Ok(host::Response::ReadFailure { index, error }) = response {
            return self.resume_read_error(mc, index, Error::from(*error));
        }
        if self.host_origin.is_none() {
            return Err(Error::new("RuntimeError", "no native call is suspended"));
        }
        let result = response.and_then(|response| self.host_value(mc, response));
        self.resume_value(mc, result)
    }
    pub fn resume_value(
        &mut self,
        mc: &Mutation<'gc>,
        result: Result<Value<'gc>, Error>,
    ) -> Result<(), Error> {
        if let Some(handle) = self
            .callback_start
            .take()
            .or_else(|| self.callback_reply.take())
        {
            let mut state = handle.borrow_mut(mc);
            if let Some(child) = state.vm.as_mut()
                && let Err(error) = child.resume_value(mc, result)
            {
                state.error = Some(error);
                state.vm = None;
            }
            return self.resume_value(mc, Ok(Value::Unit));
        }
        let (source, span) = self
            .host_origin
            .take()
            .ok_or_else(|| Error::new("RuntimeError", "no native call is suspended"))?;
        self.host_request = None;
        match result {
            Ok(value) => self.stack.push(value),
            Err(mut error) => {
                if error.origin.is_none() {
                    error.origin = Some(source);
                    error.span = Some(span);
                }
                self.fail(mc, error)?;
            }
        }
        Ok(())
    }
    pub(crate) fn resume_read_error(
        &mut self,
        mc: &Mutation<'gc>,
        index: usize,
        error: Error,
    ) -> Result<(), Error> {
        if let Some(handle) = self.callback_reply.take() {
            let mut state = handle.borrow_mut(mc);
            if let Some(child) = state.vm.as_mut()
                && let Err(error) = child.resume_read_error(mc, index, error)
            {
                state.error = Some(error);
                state.vm = None;
            }
            return self.resume_value(mc, Ok(Value::Unit));
        }
        if let Some(task) = self
            .frames
            .last_mut()
            .and_then(|frame| frame.stream.as_mut())
        {
            task.resume_read_error(mc, index, error);
            self.host_origin = None;
            self.host_request = None;
            return Ok(());
        }
        self.resume_value(mc, Err(error))
    }
    fn host_value(
        &mut self,
        mc: &Mutation<'gc>,
        response: host::Response,
    ) -> Result<Value<'gc>, Error> {
        Ok(match response {
            host::Response::JobStopped => {
                return Err(Error::new(
                    "JobStopped",
                    "job is stopped; use bg or fg to resume it",
                ));
            }
            host::Response::ReadFailure { .. }
            | host::Response::Deferred(_)
            | host::Response::Operation { .. } => {
                unreachable!("read failures are routed before conversion")
            }
            host::Response::ModuleFile(_) | host::Response::ModuleText { .. } => {
                return Err(Error::new("RuntimeError", "module response outside import"));
            }
            host::Response::Paths(paths) => operations::list(
                mc,
                paths
                    .into_iter()
                    .map(|path| Value::path(mc, path))
                    .collect::<Result<_, _>>()?,
            ),
            host::Response::Job(id) => Value::Job(id),
            host::Response::Jobs(entries) => job_snapshots(mc, entries),
            host::Response::Source(key) => {
                self.resources.push(key);
                self.resource_scopes.insert(key, self.owner());
                let value =
                    crate::stream::token(mc, self.owner(), crate::stream::Source::External(key));
                self.streams.push(value);
                value
            }
            host::Response::Through { output, input } => {
                let values = [output, input]
                    .into_iter()
                    .map(|key| self.host_value(mc, host::Response::Source(key)))
                    .collect::<Result<_, _>>()?;
                operations::list(mc, values)
            }
            host::Response::Pending => Value::Null,
            host::Response::Item {
                index,
                value: entry,
            } => {
                let value = entry.map(|item| source_item(mc, item)).transpose()?;
                operations::list(
                    mc,
                    vec![
                        Value::Int(i64::try_from(index).map_err(|_| Error::arithmetic())?),
                        operations::list(mc, value.into_iter().collect()),
                    ],
                )
            }
            host::Response::Unit => Value::Unit,
            host::Response::Text(text) => Value::string(mc, text),
            host::Response::Path(path) => Value::path(mc, path)?,
            host::Response::Plan(plan) => Value::Plan(Gc::new(mc, crate::value::JobPlan(plan))),
            host::Response::OptionalBytes(value) => operations::list(
                mc,
                value
                    .into_iter()
                    .map(|bytes| Value::bytes(mc, bytes))
                    .collect(),
            ),
            host::Response::Arguments(values) => operations::list(
                mc,
                values
                    .into_iter()
                    .map(|bytes| Value::bytes(mc, bytes))
                    .collect(),
            ),
            host::Response::Run {
                id,
                mode,
                plan,
                terminations,
                stdout,
                stderr,
                cancelled,
            } => {
                let report = self.report(mc, id, &plan, &terminations, cancelled)?;
                match mode {
                    host::RunMode::Checked => {
                        self.check_report(report)?;
                        Value::Unit
                    }
                    host::RunMode::Report => report,
                    host::RunMode::Capture => Value::Record(crate::heap::record(
                        mc,
                        [
                            ("report".into(), report),
                            ("stdout".into(), Value::bytes(mc, stdout)),
                            ("stderr".into(), Value::bytes(mc, stderr)),
                        ]
                        .into_iter()
                        .collect(),
                    )),
                }
            }
        })
    }
    fn process_namespace(&self) -> Result<Value<'gc>, Error> {
        self.modules()
            .get(&Key::Bundled("std:process"))
            .copied()
            .ok_or_else(|| Error::new("ImportError", "std:process is not initialized"))
    }
    fn report(
        &self,
        mc: &Mutation<'gc>,
        id: i64,
        plan: &rill_system::plan::Plan,
        terminations: &[rill_system::job::Termination],
        cancelled: bool,
    ) -> Result<Value<'gc>, Error> {
        use rill_system::job::Termination;
        let process = self.process_namespace()?;
        let rill_system::report::Policy { cutoff, failure } =
            rill_system::report::classify(plan, terminations);
        let stages = plan
            .stages
            .iter()
            .zip(terminations)
            .enumerate()
            .map(|(index, (stage, termination))| {
                let (case, field, code) = match termination {
                    Termination::Exited(code) => ("Exited", "code", *code),
                    Termination::Signaled(signal) => ("Signaled", "signal", *signal),
                };
                let descriptor = process.field("Termination")?.field(case)?;
                let Value::Constructor(descriptor) = descriptor else {
                    return Err(Error::type_error("invalid Termination constructor"));
                };
                let termination = nominal(mc, descriptor, [(field, Value::Int(i64::from(code)))]);
                Ok(Value::Record(crate::heap::record(
                    mc,
                    [
                        (
                            "index".into(),
                            Value::Int(i64::try_from(index).map_err(|_| Error::arithmetic())?),
                        ),
                        ("termination".into(), termination),
                        (
                            "accepted_codes".into(),
                            operations::list(
                                mc,
                                stage
                                    .accepted_codes
                                    .iter()
                                    .map(|code| Value::Int(i64::from(*code)))
                                    .collect(),
                            ),
                        ),
                        ("expected_cutoff".into(), Value::Bool(cutoff[index])),
                    ]
                    .into_iter()
                    .collect(),
                )))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let Value::Constructor(report_descriptor) = process.field("JobReport")? else {
            return Err(Error::type_error("invalid JobReport constructor"));
        };
        Ok(nominal(
            mc,
            report_descriptor,
            [
                ("id", Value::Int(id)),
                ("stages", operations::list(mc, stages)),
                (
                    "completion",
                    if cancelled {
                        nominal(
                            mc,
                            descriptor(process.field("Completion")?.field("Cancelled")?)?,
                            [("reason", Value::string(mc, "cancelled by request"))],
                        )
                    } else {
                        process.field("Completion")?.field("Finished")?
                    },
                ),
                (
                    "failure",
                    match failure {
                        Some(index) => nominal(
                            mc,
                            descriptor(
                                self.modules()[&Key::Bundled("std:core")]
                                    .field("Option")?
                                    .field("Some")?,
                            )?,
                            [(
                                "value",
                                Value::Int(i64::try_from(index).map_err(|_| Error::arithmetic())?),
                            )],
                        ),
                        None => self.modules()[&Key::Bundled("std:core")]
                            .field("Option")?
                            .field("None")?,
                    },
                ),
            ],
        ))
    }
    fn check_report(&self, report: Value<'gc>) -> Result<(), Error> {
        native::report::check(
            self.process_namespace()?,
            self.modules()[&Key::Bundled("std:core")],
            report,
        )
    }
    fn native(
        &mut self,
        mc: &Mutation<'gc>,
        intrinsic: Intrinsic,
        argument: Value<'gc>,
        tail: bool,
    ) -> Result<(), Error> {
        let result = match intrinsic {
            Intrinsic::Pure => {
                if let Some(work) = native::sort::Work::new(argument)? {
                    self.frames.last_mut().expect("native frame").work =
                        Some(Box::new(native::work::Work::Sort(work)));
                    return Ok(());
                }
                if let Some(work) = native::collection::Work::new(argument)? {
                    self.frames.last_mut().expect("native frame").work =
                        Some(Box::new(native::work::Work::Collection(work)));
                    return Ok(());
                }
                native::apply(mc, argument)?
            }
            Intrinsic::Process => {
                if let Some(request) = native::service::process(argument)? {
                    self.suspend(request);
                    return Ok(());
                }
                native::plan::apply(mc, argument)?
            }
            Intrinsic::Host(name) => {
                if name == "check" {
                    self.check_report(argument)?;
                    Value::Unit
                } else {
                    self.suspend(native::service::host(name, argument)?);
                    return Ok(());
                }
            }
            Intrinsic::Data => {
                if let Some(request) = native::service::data(argument)? {
                    self.suspend(request);
                    return Ok(());
                }
                let request = native::items(argument)?;
                if let [Value::String(name), path] = request.as_slice()
                    && name.as_str() == "display_path"
                {
                    self.stack.push(native::display_path(mc, *path)?);
                    return Ok(());
                }
                let [Value::String(name), options, input] = request.as_slice() else {
                    return Err(Error::type_error("invalid data primitive request"));
                };
                self.frames.last_mut().expect("native frame").work = Some(Box::new(
                    native::work::Work::Json(native::json::Work::new(name, *options, *input)?),
                ));
                return Ok(());
            }
            Intrinsic::Bytes => {
                let values = native::items(argument)?;
                let bytes = values
                    .as_slice()
                    .iter()
                    .map(|value| {
                        if let Value::Int(n) = value {
                            u8::try_from(*n).map_err(|_| {
                                Error::new("TypeError", "byte value must be between 0 and 255")
                            })
                        } else {
                            Err(Error::type_error("bytes requires a List of Ints"))
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Value::bytes(mc, bytes)
            }
            Intrinsic::Path => Value::path(mc, native::native_path(argument)?.to_path_buf())?,
            Intrinsic::Attempt => {
                self.attempt(argument, tail)?;
                return Ok(());
            }
            Intrinsic::Raise => return self.raise(argument),
            Intrinsic::Stream => match crate::stream::apply(mc, argument, self.owner())? {
                crate::stream::Outcome::Host(request) => {
                    self.suspend(request);
                    return Ok(());
                }
                crate::stream::Outcome::Value(value) => value,
                crate::stream::Outcome::Producer(protocol) => self.producer(mc, protocol)?,
                crate::stream::Outcome::Task(task) => {
                    self.start_stream(task, tail);
                    return Ok(());
                }
            },
        };
        if matches!(result, Value::Stream(_)) {
            self.streams.push(result);
        }
        self.stack.push(result);
        Ok(())
    }
    fn raise(&mut self, argument: Value<'gc>) -> Result<(), Error> {
        let Value::Adt(value) = argument else {
            return Err(Error::type_error("raise requires Error"));
        };
        if !Gc::ptr_eq(value.descriptor, self.descriptors()?.error) {
            return Err(Error::type_error("raise requires the shared Error type"));
        }
        let (Value::String(kind), Value::String(message), Value::List(notes)) = (
            argument.field("kind")?,
            argument.field("message")?,
            argument.field("notes")?,
        ) else {
            return Err(Error::type_error(
                "Error fields require String kind/message and List[String] notes",
            ));
        };
        if !matches!(argument.field("span")?, Value::Null | Value::Record(_))
            || notes
                .as_slice()
                .iter()
                .any(|note| !matches!(note, Value::String(_)))
        {
            return Err(Error::type_error("invalid Error span or notes"));
        }
        self.raised = Some(argument);
        let mut error = Error::new(kind.as_str(), message.as_str());
        error.notes = notes
            .as_slice()
            .iter()
            .filter_map(|note| {
                if let Value::String(note) = note {
                    Some(note.as_str().to_owned())
                } else {
                    None
                }
            })
            .collect();
        Err(error)
    }
    fn start_stream(&mut self, task: Box<crate::stream::Task<'gc>>, tail: bool) {
        let caller = self.frames.last().expect("stream caller");
        let constants = caller.constants;
        let source = Rc::clone(&caller.code.source);
        let span = caller.code.instructions[caller.ip - 1].1.clone();
        let base = if tail {
            let frame = self.frames.pop().expect("tail caller");
            self.stack.truncate(frame.stack_base);
            frame.stack_base
        } else {
            self.stack.len()
        };
        self.frames.push(Frame {
            constants,
            code: Rc::new(Code {
                source,
                context: None,
                constants: Vec::new(),
                layouts: Vec::new(),
                instructions: vec![
                    (Instruction::DriveStream, span.clone()),
                    (Instruction::Call { tail: false }, span.clone()),
                    (Instruction::Jump(0), span.clone()),
                    (Instruction::Return, span),
                ],
            }),
            ip: 0,
            scopes: Vec::new(),
            stack_base: base,
            module: None,
            exports: crate::value::Record::new(),
            attempt: None,
            plan_base: self.plans.len(),
            resource_base: self.resources.len(),
            stream_base: self.streams.len(),
            stream: Some(task),
            work: None,
            finalizer: None,
            owner: self.owner(),
            producer_base: self.producers.len(),
        });
    }
    fn drive_stream(&mut self, mc: &Mutation<'gc>, fuel: &mut usize) -> Result<(), Error> {
        use crate::stream::Step;
        let mut task = self
            .frames
            .last_mut()
            .expect("stream frame")
            .stream
            .take()
            .expect("stream task");
        if task.awaiting {
            task.resume(self.pop());
        }
        let step = task.advance(mc);
        self.frames.last_mut().expect("stream frame").stream = Some(task);
        match step? {
            Step::Yield => self.jump(0),
            Step::Callback(handle, invocation) => {
                self.jump(2);
                self.callback(mc, handle, invocation, fuel)?;
            }
            Step::DiscardCallbacks(handles) => {
                self.discard_callbacks(mc, handles);
                self.jump(2);
                if self.pending_callbacks.is_empty() && self.cancel_operations.is_empty() {
                    self.stack.push(Value::Unit);
                } else {
                    self.start_cleanup(
                        Vec::new(),
                        Vec::new(),
                        crate::stream::producer::Reason::Closed,
                        cleanup::After::Resume,
                        None,
                    );
                }
            }
            Step::Call(function, argument) => self.stack.extend([function, argument]),
            Step::ScopedCall(owner, function, argument) => {
                self.jump(2);
                self.scoped_call(owner, function, argument);
            }
            Step::Finalize(producers, reason) => {
                self.jump(2);
                self.start_cleanup(producers, Vec::new(), reason, cleanup::After::Resume, None);
            }
            Step::Host(request) => {
                self.suspend(request);
                self.jump(2);
            }
            Step::Done(value) => {
                if matches!(value, Value::Stream(_)) {
                    self.streams.push(value);
                }
                self.stack.push(value);
                self.jump(3);
            }
        }
        Ok(())
    }
    fn attempt(&mut self, thunk: Value<'gc>, tail: bool) -> Result<(), Error> {
        let descriptors = self.descriptors()?;
        let caller = self.frames.last().expect("active caller");
        let constants = caller.constants;
        let source = Rc::clone(&caller.code.source);
        let span = caller.code.instructions[caller.ip - 1].1.clone();
        let base = if tail {
            let frame = self.frames.pop().expect("tail caller");
            self.stack.truncate(frame.stack_base);
            frame.stack_base
        } else {
            self.stack.len()
        };
        self.frames.push(Frame {
            constants,
            code: Rc::new(Code {
                source,
                context: None,
                constants: Vec::new(),
                layouts: Vec::new(),
                instructions: vec![
                    (Instruction::Call { tail: false }, span.clone()),
                    (Instruction::Return, span),
                ],
            }),
            ip: 0,
            scopes: Vec::new(),
            stack_base: base,
            module: None,
            exports: crate::value::Record::new(),
            attempt: Some(descriptors),
            plan_base: self.plans.len(),
            resource_base: self.resources.len(),
            stream_base: self.streams.len(),
            stream: None,
            work: None,
            finalizer: None,
            owner: self.owner(),
            producer_base: self.producers.len(),
        });
        self.stack.extend([thunk, Value::Unit]);
        Ok(())
    }
    fn catch_error(&mut self, mc: &Mutation<'gc>, error: &Error) -> bool {
        let Some(index) = self
            .error_checkpoint()
            .filter(|index| self.frames[*index].attempt.is_some())
        else {
            return false;
        };
        let frame = self.frames.remove(index);
        self.finished_contexts.extend(
            self.frames
                .drain(index..)
                .filter_map(|frame| frame.code.context),
        );
        for value in self.streams.drain(frame.stream_base..) {
            if let Value::Stream(token) = value {
                token.borrow_mut(mc).source.take();
            }
        }
        self.plans.truncate(frame.plan_base);
        self.stack.truncate(frame.stack_base);
        let attempt = frame.attempt.expect("error checkpoint");
        let value = self.raised.take().map_or_else(
            || error_value(mc, attempt.error, error),
            |value| error_notes(mc, value, &error.notes),
        );
        self.stack
            .push(nominal(mc, attempt.err, [("error", value)]));
        true
    }
    fn call(&mut self, mc: &Mutation<'gc>, tail: bool) -> Result<(), Error> {
        let argument = self.pop();
        let function = self.pop();
        if let Value::Native(intrinsic) = function {
            return self.native(mc, intrinsic, argument, tail);
        }
        if let Value::Constructor(descriptor) = function {
            self.stack.push(construct(mc, descriptor, argument)?);
            return Ok(());
        }
        let Value::Function(closure) = function else {
            return Err(Error::type_error(format!(
                "cannot apply a {} value",
                function.kind()
            )));
        };
        let mut environment = closure.environment.borrow().clone();
        environment.extend(pattern::bind(
            mc,
            &closure.code.parameters[closure.parameter],
            argument,
            &|name| {
                environment
                    .get(name)
                    .copied()
                    .ok_or_else(|| Error::new("NameError", format!("unknown constructor '{name}'")))
            },
        )?);
        if closure.parameter + 1 < closure.code.parameters.len() {
            self.stack.push(Value::Function(Gc::new(
                mc,
                Closure {
                    constants: closure.constants,
                    code: Rc::clone(&closure.code),
                    parameter: closure.parameter + 1,
                    environment: Gc::new(mc, RefLock::new(environment)),
                },
            )));
        } else {
            let base = if tail {
                let old = self.frames.pop().expect("active frame");
                self.stack.truncate(old.stack_base);
                old.stack_base
            } else {
                self.stack.len()
            };
            if self.frames.len() >= 65_536 {
                return Err(Error::new(
                    "LimitExceeded",
                    "live continuation limit exceeded",
                ));
            }
            self.frames.push(Frame {
                constants: closure.constants,
                code: Rc::clone(&closure.code.body),
                ip: 0,
                scopes: vec![environment],
                stack_base: base,
                module: None,
                exports: crate::value::Record::new(),
                attempt: None,
                plan_base: self.plans.len(),
                resource_base: self.resources.len(),
                stream_base: self.streams.len(),
                stream: None,
                work: None,
                finalizer: None,
                owner: self.owner(),
                producer_base: self.producers.len(),
            });
        }
        Ok(())
    }
}
fn boolean(value: Value<'_>) -> Result<bool, Error> {
    if let Value::Bool(value) = value {
        Ok(value)
    } else {
        Err(Error::type_error("condition requires Bool"))
    }
}

fn constructor<'gc>(mc: &Mutation<'gc>, name: String, fields: Vec<String>) -> Value<'gc> {
    let descriptor = Gc::new(mc, Descriptor { name, fields });
    if descriptor.fields.is_empty() {
        Value::Adt(Gc::new(
            mc,
            Adt {
                descriptor,
                fields: Gc::new(mc, crate::value::Record::new()),
            },
        ))
    } else {
        Value::Constructor(descriptor)
    }
}

fn nominal<'gc, const N: usize>(
    mc: &Mutation<'gc>,
    descriptor: Gc<'gc, Descriptor>,
    fields: [(&str, Value<'gc>); N],
) -> Value<'gc> {
    Value::Adt(Gc::new(
        mc,
        Adt {
            descriptor,
            fields: crate::heap::record(
                mc,
                fields
                    .into_iter()
                    .map(|(name, value)| (name.into(), value))
                    .collect(),
            ),
        },
    ))
}
fn construct<'gc>(
    mc: &Mutation<'gc>,
    descriptor: Gc<'gc, Descriptor>,
    argument: Value<'gc>,
) -> Result<Value<'gc>, Error> {
    let Value::Record(fields) = argument else {
        return Err(Error::type_error(
            "a constructor requires an anonymous Record",
        ));
    };
    if fields.len() != descriptor.fields.len()
        || descriptor
            .fields
            .iter()
            .any(|name| !fields.contains_key(name))
    {
        return Err(Error::type_error(
            "constructor fields do not match the declared shape",
        ));
    }
    let fields = descriptor
        .fields
        .iter()
        .map(|name| (name.clone(), fields[name]))
        .collect();
    Ok(Value::Adt(Gc::new(
        mc,
        Adt {
            descriptor,
            fields: crate::heap::record(mc, fields),
        },
    )))
}

fn descriptor(value: Value<'_>) -> Result<Gc<'_, Descriptor>, Error> {
    if let Value::Constructor(descriptor) = value {
        Ok(descriptor)
    } else {
        Err(Error::type_error("invalid nominal constructor"))
    }
}

fn job_snapshots<'gc>(mc: &Mutation<'gc>, entries: Vec<host::JobSnapshot>) -> Value<'gc> {
    operations::list(
        mc,
        entries
            .into_iter()
            .map(|host::JobSnapshot { id, kind, state }| {
                Value::Record(crate::heap::record(
                    mc,
                    [
                        ("id".into(), Value::Int(id)),
                        ("handle".into(), Value::Job(id)),
                        ("kind".into(), Value::string(mc, kind)),
                        ("state".into(), Value::string(mc, state)),
                    ]
                    .into_iter()
                    .collect(),
                ))
            })
            .collect(),
    )
}

fn source_item<'gc>(
    mc: &Mutation<'gc>,
    item: rill_system::resources::Item,
) -> Result<Value<'gc>, Error> {
    let entry = match item {
        rill_system::resources::Item::Written(open) => return Ok(Value::Bool(open)),
        rill_system::resources::Item::Bytes(bytes) => return Ok(Value::bytes(mc, bytes)),
        rill_system::resources::Item::Entry(entry) => entry,
    };
    Ok(Value::Record(crate::heap::record(
        mc,
        [
            ("name".into(), Value::path(mc, entry.name)?),
            ("path".into(), Value::path(mc, entry.path)?),
            ("kind".into(), Value::string(mc, entry.kind)),
            ("size".into(), Value::Int(entry.size)),
        ]
        .into_iter()
        .collect(),
    )))
}

fn error_notes<'gc>(mc: &Mutation<'gc>, value: Value<'gc>, notes: &[String]) -> Value<'gc> {
    let Value::Adt(raised) = value else {
        unreachable!("error value")
    };
    let mut fields = (*raised.fields).clone();
    fields.insert(
        "notes".into(),
        operations::list(
            mc,
            notes
                .iter()
                .map(|note| Value::string(mc, note.clone()))
                .collect(),
        ),
    );
    Value::Adt(Gc::new(
        mc,
        Adt {
            descriptor: raised.descriptor,
            fields: Gc::new(mc, fields),
        },
    ))
}

fn error_value<'gc>(
    mc: &Mutation<'gc>,
    descriptor: Gc<'gc, Descriptor>,
    error: &Error,
) -> Value<'gc> {
    let span =
        error
            .origin
            .as_ref()
            .zip(error.span.as_ref())
            .map_or(Value::Null, |(source, span)| {
                Value::Record(crate::heap::record(
                    mc,
                    [
                        ("source".into(), Value::string(mc, source.name.clone())),
                        (
                            "offset".into(),
                            Value::Int(i64::try_from(span.start).unwrap_or(i64::MAX)),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                ))
            });
    nominal(
        mc,
        descriptor,
        [
            ("kind", Value::string(mc, error.kind.clone())),
            ("message", Value::string(mc, error.message.clone())),
            ("span", span),
            (
                "notes",
                operations::list(
                    mc,
                    error
                        .notes
                        .iter()
                        .map(|note| Value::string(mc, note.clone()))
                        .collect(),
                ),
            ),
        ],
    )
}
