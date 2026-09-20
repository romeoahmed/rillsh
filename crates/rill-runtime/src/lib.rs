//! Compiled Rill evaluation with explicit VM continuations and a traced heap.
//!
//! [`Engine::standard`] loads the embedded library; [`Engine::default`] supplies only
//! the language core. Drive entries with [`Engine::step`], service [`Progress::Waiting`]
//! through [`Engine::take_request`] and [`Engine::resume`], and inspect completed results.
//! Dropping an engine does not run Rill cleanup callbacks: its owner must finish or
//! interrupt active and parked entries before releasing their OS resources.
//!
//! ```
//! use rill_runtime::{Engine, Progress, value::Value};
//!
//! let module = rill_syntax::parse("example", "{ n => n + 1 } 41").unwrap();
//! let mut engine = Engine::default();
//! engine.begin(&module).unwrap();
//! loop {
//!     match engine.step(64).unwrap() {
//!         Progress::Complete => break,
//!         Progress::Yielded => {}
//!         Progress::Waiting => panic!("this example has no host effects"),
//!     }
//! }
//! assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
//! ```
mod code;
mod compiler;
mod completion;
mod equality;
mod error;
mod heap;
pub mod host;
mod modules;
mod native;
mod operations;
mod pattern;
pub mod presentation;
mod scope;
mod stream;
pub mod value;
mod vm;
pub use code::Source;
pub use error::Error;
use gc_arena::{Arena, Collect, Rootable};
use rill_syntax::ast::Module;
use value::Value;

/// Why a bounded VM quantum returned control to its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// The entry committed its bindings and result.
    Complete,
    /// More instructions or native work can run after scheduling and collection.
    Yielded,
    /// A host request must be serviced before execution can continue.
    Waiting,
}

#[derive(Collect, Default)]
#[collect(no_drop)]
struct State<'gc> {
    current: vm::Vm<'gc>,
    suspended: std::collections::HashMap<i64, vm::Vm<'gc>>,
}

/// Interpreter state with no GC pointers exposed between mutation callbacks.
pub struct Engine {
    arena: Arena<Rootable![State<'_>]>,
    loader: modules::Loader,
    importing: Option<modules::Request>,
    import_work: Option<host::Request>,
    parked_imports: std::collections::HashMap<i64, (modules::Request, Option<host::Request>)>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            arena: Arena::new(|_| State::default()),
            loader: modules::Loader::default(),
            importing: None,
            import_work: None,
            parked_imports: std::collections::HashMap::new(),
        }
    }
}

impl Engine {
    /// Initialize the bundled prelude through ordinary module evaluation.
    ///
    /// # Errors
    /// Reports bundled initialization failures; no partially initialized engine escapes.
    pub fn standard() -> Result<Self, Error> {
        let mut engine = Self::default();
        let source = rill_syntax::parse("<bootstrap>", "import \"std:prelude\" as prelude")
            .map_err(|errors| Error::new("ParseError", errors[0].message.clone()))?;
        engine.begin(&source)?;
        while engine.step(1024)? != Progress::Complete {}
        engine
            .arena
            .mutate_root(|_, state| state.current.install_prelude());
        Ok(engine)
    }

    /// Compile and begin an entry; bindings publish only on successful completion.
    ///
    /// # Errors
    /// Rejects an active evaluation or an inaccessible base directory for file imports.
    pub fn begin(&mut self, module: &Module) -> Result<(), Error> {
        self.begin_in(module, std::path::Path::new("."))
    }
    /// Begin an entry with an explicit base directory for relative imports.
    ///
    /// # Errors
    /// Rejects an active evaluation or an inaccessible import directory.
    pub fn begin_in(&mut self, module: &Module, directory: &std::path::Path) -> Result<(), Error> {
        let directory = if modules::needs_directory(module) {
            Some(rill_system::source::Directory::open(directory)?)
        } else {
            None
        };
        self.begin_with_directory(module, directory)
    }
    /// Begin with an owned import capability that remains valid after directory renames.
    ///
    /// # Errors
    /// Rejects an active evaluation before changing engine state.
    pub fn begin_at(
        &mut self,
        module: &Module,
        directory: rill_system::source::Directory,
    ) -> Result<(), Error> {
        self.begin_with_directory(module, Some(directory))
    }
    /// Begin an interactive entry, draining a returned stream before publishing bindings.
    /// Display requests contain escaped text and use the ordinary host suspension path.
    ///
    /// # Errors
    /// Rejects an active evaluation before changing engine state.
    pub fn begin_interactive_at(
        &mut self,
        module: &Module,
        directory: rill_system::source::Directory,
    ) -> Result<(), Error> {
        self.begin_at(module, directory)?;
        self.arena
            .mutate_root(|_, state| state.current.display = true);
        Ok(())
    }
    fn begin_with_directory(
        &mut self,
        module: &Module,
        directory: Option<rill_system::source::Directory>,
    ) -> Result<(), Error> {
        if self.arena.mutate(|_, state| state.current.active()) {
            return Err(Error::new(
                "RuntimeBusy",
                "finish or interrupt the active entry before beginning another",
            ));
        }
        let context = directory.and_then(|directory| self.loader.context(module, directory));
        let code = compiler::compile(module, context);
        self.arena
            .mutate_root(|mc, state| state.current.begin(mc, code));
        self.release_contexts();
        self.loader.abort();
        Ok(())
    }
    /// Spend at most `fuel` instruction/native-work units, then service collection debt.
    /// Distinguishes completion, a cooperative yield, and a pending host request.
    /// Fuel bounds cooperative work units, not elapsed time, allocation or GC pauses.
    ///
    /// # Errors
    /// Returns the entry's language error and discards its unpublished bindings.
    pub fn step(&mut self, fuel: usize) -> Result<Progress, Error> {
        if self.importing.is_some() {
            return Ok(Progress::Waiting);
        }
        let result = self
            .arena
            .mutate_root(|mc, state| state.current.step(mc, fuel));
        let complete = self.settle(result)?;
        if let Some(request) = self
            .arena
            .mutate_root(|_, state| state.current.request.take())
        {
            if let Err(mut error) = self.import(&request) {
                if error.origin.is_none() {
                    error.origin = Some(request.source);
                    error.span = Some(request.span);
                }
                let result = self
                    .arena
                    .mutate_root(|mc, state| state.current.fail(mc, error));
                self.settle(result)?;
            } else if self.import_work.is_some() {
                self.importing = Some(request);
                return Ok(Progress::Waiting);
            }
            return Ok(Progress::Yielded);
        }
        Ok(complete)
    }
    /// Begin uncatchable cancellation, retaining cleanup callbacks until subsequent quanta finish.
    ///
    /// # Errors
    /// Returns cancellation immediately if no cleanup is needed.
    pub fn interrupt(&mut self) -> Result<(), Error> {
        self.importing = None;
        self.import_work = None;
        let result = self
            .arena
            .mutate_root(|mc, state| state.current.interrupt(mc));
        self.settle(result)
    }
    /// Finalize the current entry for session exit without publishing pending bindings.
    ///
    /// # Errors
    /// Reports cleanup failure or the internal `SessionExit` completion marker.
    pub fn finish(&mut self) -> Result<(), Error> {
        self.importing = None;
        self.import_work = None;
        let result = self
            .arena
            .mutate_root(|mc, state| state.current.stop(mc, false));
        self.settle(result)
    }
    /// Whether explicit cleanup currently protects release callbacks from interruption.
    #[must_use]
    pub fn cleaning(&self) -> bool {
        self.arena.mutate(|_, state| state.current.cleaning())
    }
    fn settle<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        self.release_contexts();
        self.arena.collect_debt();
        if result.is_err() {
            self.loader.abort();
        }
        result
    }
    fn release_contexts(&mut self) {
        let contexts = self
            .arena
            .mutate_root(|_, state| std::mem::take(&mut state.current.finished_contexts));
        self.loader.release(contexts);
        let published = self
            .arena
            .mutate_root(|_, state| std::mem::take(&mut state.current.published_modules));
        self.loader.publish(published);
    }
    fn imported(&mut self, key: &modules::Key, name: &str) -> Result<bool, Error> {
        self.arena.mutate_root(|_, state| {
            if state.suspended.values().any(|vm| vm.importing(key)) {
                return Err(Error::new("ImportBusy", "module initialization belongs to a suspended evaluation; resume or cancel it first"));
            }
            state.current.imported(key, name)
        })
    }
    /// Raise a host failure through the current continuation's ordinary cleanup path.
    ///
    /// # Errors
    /// Returns an uncaught failure after synchronous cleanup; otherwise keep stepping.
    pub fn fail(&mut self, error: Error) -> Result<(), Error> {
        self.importing = None;
        self.import_work = None;
        let result = self
            .arena
            .mutate_root(|mc, state| state.current.fail(mc, error));
        self.settle(result)
    }
    fn import(&mut self, request: &modules::Request) -> Result<(), Error> {
        if request.path.starts_with("std:") {
            let (name, source) = modules::bundle(&request.path).ok_or_else(|| {
                Error::new(
                    "ImportError",
                    format!("unknown bundled module '{}'", request.path),
                )
            })?;
            let key = modules::Key::Bundled(name);
            if self.imported(&key, &request.name)? {
                return Ok(());
            }
            let module = rill_syntax::parse(name, source)
                .map_err(|errors| Error::new("ParseError", errors[0].message.clone()))?;
            let code = compiler::compile(&module, None);
            self.arena.mutate_root(|mc, state| {
                state
                    .current
                    .begin_module(mc, key, request.name.clone(), code);
            });
            return Ok(());
        }
        self.import_work = Some(host::Request::OpenModule {
            directory: self.loader.directory(request)?,
            path: request.path.clone().into(),
        });
        Ok(())
    }
    /// Take the pending owned service request. Evaluation remains suspended until [`Self::resume`].
    /// Keep the request's operation and resources alive across scheduling or suspension.
    pub fn take_request(&mut self) -> Option<host::Request> {
        self.import_work.take().or_else(|| {
            self.arena
                .mutate_root(|_, state| state.current.host_request.take())
        })
    }
    /// Resume a suspended native call with a service result or language error.
    ///
    /// # Errors
    /// Reports an uncaught service error or a response outside a pending call.
    pub fn resume(&mut self, response: Result<host::Response, Error>) -> Result<(), Error> {
        if let Some(request) = self.importing.take() {
            let result = self.resume_import(&request, response);
            if self.import_work.is_some() {
                self.importing = Some(request);
            } else if let Err(mut error) = result {
                if error.origin.is_none() {
                    error.origin = Some(request.source);
                    error.span = Some(request.span);
                }
                return self.fail(error);
            }
            return self.settle(Ok(()));
        }
        let result = self
            .arena
            .mutate_root(|mc, state| state.current.resume(mc, response));
        self.settle(result)
    }
    fn resume_import(
        &mut self,
        request: &modules::Request,
        response: Result<host::Response, Error>,
    ) -> Result<(), Error> {
        match response? {
            host::Response::ModuleFile(file) => {
                if !self.imported(&modules::Key::File(file.identity), &request.name)? {
                    self.import_work = Some(host::Request::ReadModule(file));
                }
            }
            host::Response::ModuleText { file, text } => {
                let key = modules::Key::File(file.identity);
                let code = self.loader.compile(request, file, text)?;
                self.arena.mutate_root(|mc, state| {
                    state
                        .current
                        .begin_module(mc, key, request.name.clone(), code);
                });
            }
            _ => {
                return Err(Error::new(
                    "RuntimeError",
                    "invalid module service response",
                ));
            }
        }
        Ok(())
    }
    /// Park the active continuation in the same traced arena, leaving room for a new entry.
    /// Session bindings and module identity remain shared; lexical snapshots stay with frames.
    /// The host must retain the matching service operation and OS resources until activation
    /// or explicit cancellation; parking does not restart or release them.
    ///
    /// # Errors
    /// Rejects an inactive entry or an identity already retained by this engine.
    pub fn suspend(&mut self, id: i64) -> Result<(), Error> {
        self.arena.mutate_root(|_, state| {
            if !state.current.active() || state.suspended.contains_key(&id) {
                return Err(Error::new(
                    "RuntimeError",
                    "invalid continuation suspension",
                ));
            }
            let mut idle = vm::Vm::default();
            state.current.transfer_session(&mut idle);
            let parked = std::mem::replace(&mut state.current, idle);
            state.suspended.insert(id, parked);
            Ok(())
        })?;
        self.loader.suspend(id);
        if let Some(request) = self.importing.take() {
            self.parked_imports
                .insert(id, (request, self.import_work.take()));
        }
        self.arena.collect_debt();
        Ok(())
    }
    /// Activate a retained continuation after the host restores its resource ownership.
    ///
    /// # Errors
    /// Rejects a missing identity or replacement of an active continuation.
    pub fn activate(&mut self, id: i64) -> Result<(), Error> {
        self.arena.mutate_root(|_, state| {
            if state.current.active() {
                return Err(Error::new("RuntimeBusy", "an evaluation is still active"));
            }
            let mut resumed = state
                .suspended
                .remove(&id)
                .ok_or_else(|| Error::new("JobError", "evaluation is not suspended"))?;
            state.current.transfer_session(&mut resumed);
            state.current = resumed;
            Ok(())
        })?;
        self.loader.activate(id);
        if let Some((request, work)) = self.parked_imports.remove(&id) {
            self.importing = Some(request);
            self.import_work = work;
        }
        Ok(())
    }
    /// Deliver the completed evaluation's value directly to a parked `fg` caller.
    /// The value never leaves the traced arena; errors retain their original source.
    ///
    /// # Errors
    /// Rejects an active callee, missing caller, or an uncaught resumed error.
    pub fn return_to(&mut self, caller: i64, result: Result<(), Error>) -> Result<(), Error> {
        self.arena.mutate(|_, state| {
            if state.current.active() {
                return Err(Error::new("RuntimeBusy", "callee has not completed"));
            }
            if !state.suspended.contains_key(&caller) {
                return Err(Error::new("JobError", "caller is not suspended"));
            }
            Ok(())
        })?;
        self.loader.activate(caller);
        let result = self.arena.mutate_root(|mc, state| {
            let mut resumed = state
                .suspended
                .remove(&caller)
                .ok_or_else(|| Error::new("JobError", "caller is not suspended"))?;
            let value = state.current.result;
            state.current.transfer_session(&mut resumed);
            state.current = resumed;
            state.current.resume_value(mc, result.map(|()| value))
        });
        self.settle(result)
    }
    /// Suppress automatic display when a foreground caller consumes the resumed value.
    pub fn consume_result(&mut self) {
        self.arena
            .mutate_root(|_, state| state.current.display = false);
    }
    /// Read bounded, owned completion metadata from published bindings only.
    /// Field traversal inspects materialized records and nominal values, never callbacks.
    #[must_use]
    pub fn complete(&self, query: &rill_syntax::completion::Query) -> Vec<(String, String)> {
        self.arena
            .mutate(|_, state| completion::members(&state.current.globals, query))
    }
    /// Inspect a completed value without allowing arena references to escape.
    pub fn inspect<T>(&self, inspect: impl for<'gc> FnOnce(Value<'gc>) -> T) -> T {
        self.arena.mutate(|_, state| inspect(state.current.result))
    }
    /// Finish a collection cycle, including suspended VM state.
    pub fn collect(&mut self) {
        self.arena.finish_cycle();
    }
}
