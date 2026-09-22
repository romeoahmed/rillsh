//! One coordinator owns VM quanta, external requests, and explicit child cleanup.
mod continuation;
mod editor;
mod execution;
mod files;
mod glob;
mod jobs;
mod launch;
mod operations;
mod output;
use rill_runtime::{
    Engine, Error, Progress,
    host::{Request, Response},
};
use rill_system::{
    job::{LaunchMode, Snapshot},
    plan::Plan,
};
use std::{fmt::Write as _, io, path::PathBuf};
use tokio::signal::unix::{Signal, SignalKind, signal};

pub struct Session {
    pub engine: Engine,
    snapshot: Snapshot,
    launcher: PathBuf,
    arguments: Vec<Vec<u8>>,
    interrupt: Signal,
    stop: Option<Signal>,
    suspend_requested: bool,
    pending: Option<continuation::Pending>,
    suspended: std::collections::BTreeMap<i64, continuation::Suspended>,
    active_evaluations: Vec<i64>,
    resumers: Vec<i64>,
    pub exit: Option<u8>,
    interrupted: bool,
    jobs: jobs::Jobs,
    child_events: Signal,
    sources: rill_system::resources::Sources,
    writers: output::Writers,
    operations: operations::Operations,
    terminal: Option<rill_system::terminal::Terminal>,
}
#[derive(Clone, Copy)]
pub enum InputEvent {
    Interrupt,
    Suspend,
}
impl Session {
    pub fn prompt_context(&mut self, status: u8) -> String {
        let directory = self
            .snapshot
            .environment
            .get(c"PWD")
            .map_or_else(String::new, |value| {
                String::from_utf8_lossy(value.as_bytes())
                    .chars()
                    .take(160)
                    .flat_map(char::escape_debug)
                    .collect()
            });
        let mut context = directory;
        if status != 0 {
            write!(context, "  exit {status}").expect("String formatting is infallible");
        }
        if let Ok(jobs) = self.job_snapshots() {
            let stopped = jobs.iter().filter(|job| job.state == "stopped").count();
            if stopped != 0 {
                write!(context, "  {stopped} stopped").expect("String formatting is infallible");
            }
        }
        context
    }
    pub fn completion_context(&self) -> io::Result<(PathBuf, Snapshot)> {
        Ok((
            self.launcher.clone(),
            Snapshot {
                cwd: self.snapshot.cwd.try_clone()?,
                environment: self.snapshot.environment.clone(),
            },
        ))
    }
    pub fn new(arguments: Vec<Vec<u8>>) -> Result<Self, Error> {
        Ok(Self {
            engine: Engine::standard()?,
            snapshot: Snapshot::current()?,
            launcher: std::env::current_exe()?,
            arguments,
            interrupt: signal(SignalKind::interrupt())?,
            stop: None,
            suspend_requested: false,
            pending: None,
            suspended: std::collections::BTreeMap::new(),
            active_evaluations: Vec::new(),
            resumers: Vec::new(),
            exit: None,
            interrupted: false,
            jobs: jobs::Jobs::default(),
            child_events: signal(SignalKind::child())?,
            sources: rill_system::resources::Sources::default(),
            writers: output::Writers::default(),
            operations: operations::Operations::default(),
            terminal: None,
        })
    }
    pub const fn terminal(&self) -> &rill_system::terminal::Terminal {
        self.terminal
            .as_ref()
            .expect("interactive session owns its controlling terminal")
    }
    pub fn terminal_hints(&self) -> rill_editor::profile::Hints<'_> {
        let value = |key: &std::ffi::CStr| {
            self.snapshot
                .environment
                .get(key)
                .map_or(b"".as_slice(), |value| value.as_bytes())
        };
        rill_editor::profile::Hints {
            term: value(c"TERM"),
            colorterm: value(c"COLORTERM"),
            no_color: value(c"NO_COLOR"),
        }
    }
    pub fn color(
        &self,
        choice: super::Color,
        is_terminal: bool,
    ) -> rill_editor::profile::ColorDepth {
        self.terminal_hints().color(choice.into(), is_terminal)
    }
    pub fn enable_interaction(&mut self) -> Result<(), Error> {
        self.stop = Some(signal(SignalKind::from_raw(
            rustix::process::Signal::TSTP.as_raw(),
        ))?);
        self.terminal = Some(rill_system::terminal::Terminal::open()?);
        Ok(())
    }
    #[expect(
        clippy::future_not_send,
        reason = "The traced VM and its coordinator run exclusively on the current-thread runtime"
    )]
    pub async fn input_event(&mut self, notifications: &rill_editor::Notifications) -> InputEvent {
        loop {
            // A prompt transition may cancel the previous wait while the printer is full.
            // Reconcile retained job state before waiting for another signal.
            let pending = match self.jobs.notify(notifications) {
                Ok(pending) => pending,
                Err(error) => !notifications.try_print(format!("rillsh: {error}")),
            };
            tokio::select! {
                _ = self.interrupt.recv() => return InputEvent::Interrupt,
                _ = self.child_events.recv() => {},
                () = stop_event(&mut self.stop) => return InputEvent::Suspend,
                () = tokio::time::sleep(std::time::Duration::from_millis(100)), if pending => {},
            }
        }
    }
    #[expect(
        clippy::future_not_send,
        reason = "The traced VM and its coordinator run exclusively on the current-thread runtime"
    )]
    pub async fn evaluate(
        &mut self,
        module: &rill_syntax::ast::Module,
        directory: Option<&std::path::Path>,
        display: bool,
    ) -> Result<(), Error> {
        let result = match self.drive(module, directory, display).await {
            Err(error) if error.is_exit() => Ok(()),
            result => result,
        };
        if result.as_ref().is_err_and(Error::is_cancelled) {
            // Interrupts received while cleanup was protected belong to this entry,
            // not to the next editor prompt. Tokio coalesces these notifications.
            std::future::poll_fn(|cx| {
                while matches!(
                    self.interrupt.poll_recv(cx),
                    std::task::Poll::Ready(Some(()))
                ) {}
                std::task::Poll::Ready(())
            })
            .await;
        }
        self.finish_resources(result).await
    }
    #[expect(
        clippy::future_not_send,
        reason = "The traced VM is driven only on the current-thread runtime"
    )]
    async fn drive(
        &mut self,
        module: &rill_syntax::ast::Module,
        directory: Option<&std::path::Path>,
        display: bool,
    ) -> Result<(), Error> {
        self.interrupted = false;
        let directory = rill_system::source::Directory::relative(
            &self.snapshot.cwd,
            directory.unwrap_or_else(|| std::path::Path::new(".")),
        )?;
        if display {
            self.engine.begin_interactive_at(module, directory)?;
        } else {
            self.engine.begin_at(module, directory)?;
        }
        self.drive_current().await
    }
    #[expect(
        clippy::future_not_send,
        reason = "The coordinator owns all traced continuations"
    )]
    async fn drive_current(&mut self) -> Result<(), Error> {
        let mut stopping = false;
        loop {
            if let Some(pending) = self.pending.take() {
                let response = match pending {
                    continuation::Pending::Reply(response) => response,
                    continuation::Pending::Output { stderr } => self.continue_output(stderr).await,
                    continuation::Pending::Run(run) => self.continue_run(*run).await,
                    continuation::Pending::Launch(launch) => self.continue_launch(*launch).await,
                    continuation::Pending::Glob(run) => {
                        let response = self.continue_run(*run).await;
                        response.and_then(|response| self.glob_response(response))
                    }
                    continuation::Pending::Control { id, operation } => {
                        self.control(id, operation).await
                    }
                    continuation::Pending::Read { keys, wait } => {
                        self.read_sources(keys, wait).await
                    }
                };
                if self.suspend_requested {
                    return self.park().await;
                }
                if self.interrupted && !stopping {
                    stopping = true;
                    self.interrupted = false;
                    self.cancel_entry()?;
                } else {
                    self.engine.resume(response)?;
                }
            }
            if self.engine.step(1024)? == Progress::Complete {
                return Ok(());
            }
            if let Some(request) = self.engine.take_request() {
                let control = match &request {
                    Request::Control { id, operation } => Some((*id, *operation)),
                    Request::Defer(request) => match **request {
                        Request::Control { id, operation } => Some((id, operation)),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((id, operation)) = control
                    && self.suspended.contains_key(&id)
                {
                    self.control_evaluation(id, operation).await?;
                    continue;
                }
                let response = self.service(request).await;
                if self.suspend_requested {
                    return self.park().await;
                }
                if self.interrupted && !stopping {
                    stopping = true;
                    self.interrupted = false;
                    if self.engine.cleaning() {
                        self.engine.resume(response)?;
                    }
                    self.cancel_entry()?;
                } else if self.exit.is_some() && !stopping {
                    stopping = true;
                    if self.engine.cleaning() {
                        self.engine.resume(response)?;
                    }
                    self.engine.finish()?;
                } else {
                    self.engine.resume(response)?;
                }
            }
            tokio::select! {
                biased;
                _ = self.interrupt.recv(), if !stopping && !self.engine.cleaning() => {
                    stopping = true;
                    self.cancel_entry()?;
                }
                () = stop_event(&mut self.stop), if self.terminal.is_some() && !stopping && !self.engine.cleaning() => {
                    return self.park().await;
                }
                () = tokio::task::yield_now() => {}
            }
        }
    }
    fn cancel_entry(&mut self) -> Result<(), Error> {
        if let Some(terminal) = &self.terminal {
            // A failed repaint must not prevent resource cleanup or child reaping.
            let _ = terminal.finish_line();
        }
        self.engine.interrupt()
    }
    #[expect(
        clippy::future_not_send,
        reason = "The traced VM and its coordinator run exclusively on the current-thread runtime"
    )]
    async fn service(&mut self, request: Request) -> Result<Response, Error> {
        match request {
            Request::Defer(request) => self.defer(*request).await,
            Request::OpenModule { directory, path } => {
                self.file_work(move || directory.source(&path).map(Response::ModuleFile))
                    .await
            }
            Request::ReadModule(file) => self.file_work(move || files::module(file)).await,
            Request::Glob(pattern) => self.glob(pattern).await,
            Request::Start(plan) => self.launch(plan, LaunchMode::Background).await,
            Request::Jobs => self.job_snapshots().map(Response::Jobs),
            Request::Control { id, operation } => self.control(id, operation).await,
            Request::Stdin => {
                if self.stdin_leased() || self.operations.reads_stdin() {
                    return Err(Error::new(
                        "ResourceBusy",
                        "stdin already has a live source",
                    ));
                }
                self.sources
                    .stdin()
                    .map(Response::Source)
                    .map_err(Error::from)
            }
            Request::OpenFile { path, append } => self
                .sources
                .file(self.snapshot.cwd.try_clone()?, path, append)
                .await
                .map(Response::Source)
                .map_err(Error::from),
            Request::Files(path) => self
                .sources
                .directory(self.snapshot.cwd.try_clone()?, path)
                .await
                .map(Response::Source)
                .map_err(Error::from),
            Request::Stream(plan) => self.launch(plan, LaunchMode::Stream).await,
            Request::Through(plan) => self.launch(plan, LaunchMode::Through).await,
            Request::Send { key, bytes } => self
                .sources
                .send(key, bytes)
                .map(|()| Response::Unit)
                .map_err(Error::from),
            Request::ReadSources { keys, wait } => self.read_sources(keys, wait).await,
            Request::Close(keys) => self.close_sources(keys).await,
            Request::Run {
                plan,
                mode,
                max_bytes,
            } => self.run(plan, mode, max_bytes).await,
            Request::Write(bytes) => self.output(bytes, false).await,
            Request::WriteError(bytes) => self.output(bytes, true).await,
            Request::Display(text) => self.display(text).await,
            Request::ReadText { path, max_bytes } => {
                let cwd = self.snapshot.cwd.try_clone()?;
                self.file_work(move || files::text(&cwd, &path, max_bytes))
                    .await
            }
            Request::WithCwd { path, plan } => self.with_cwd(&path, plan),
            Request::Cd(path) => self.cd(&path),
            Request::Pwd => rill_system::source::directory_path(&self.snapshot.cwd)
                .map(Response::Path)
                .map_err(Error::from),
            Request::GetEnv(name) => Ok(Response::OptionalBytes(
                self.snapshot
                    .environment
                    .get(&name)
                    .map(|v| v.as_bytes().to_vec()),
            )),
            Request::SetEnv { name, value } => {
                self.snapshot.environment.insert(name, value);
                Ok(Response::Unit)
            }
            Request::UnsetEnv(name) => {
                self.snapshot.environment.remove(&name);
                Ok(Response::Unit)
            }
            Request::Args => Ok(Response::Arguments(self.arguments.clone())),
            Request::Exit { code, force } => {
                if !force && (self.jobs.live()? || !self.suspended.is_empty()) {
                    return Err(Error::new(
                        "JobsActive",
                        "jobs are still running or stopped; finish them or use exit_force",
                    ));
                }
                if force {
                    self.jobs.shutdown().await?;
                }
                self.exit = Some(code);
                Ok(Response::Unit)
            }
        }
    }
    fn cd(&mut self, path: &std::path::Path) -> Result<Response, Error> {
        self.snapshot.set_cwd(path)?;
        Ok(Response::Unit)
    }
    fn with_cwd(&self, path: &std::path::Path, mut plan: Plan) -> Result<Response, Error> {
        let path = rill_system::source::Directory::relative(&self.snapshot.cwd, path)
            .and_then(|directory| directory.path())?;
        for stage in &mut plan.stages {
            stage.cwd = Some(path.clone());
        }
        Ok(Response::Plan(plan))
    }
    fn check_input_lease(&self, plan: &Plan, capture: bool) -> Result<(), Error> {
        if !capture
            && self.stdin_leased()
            && plan.stages.first().is_some_and(|stage| {
                !stage
                    .redirects
                    .iter()
                    .any(|redirect| matches!(redirect, rill_system::plan::Redirect::Read(_)))
            })
        {
            return Err(Error::new(
                "ResourceBusy",
                "a stream owns standard input; close it or redirect command input",
            ));
        }
        Ok(())
    }
}
async fn stop_event(signal: &mut Option<Signal>) {
    if let Some(signal) = signal {
        signal.recv().await;
    } else {
        std::future::pending::<()>().await;
    }
}
