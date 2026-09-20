//! Foreground waits retain transports and partial captures across interactive suspension.
use super::Session;
use rill_runtime::{
    Error,
    host::{Response, RunMode},
};
use rill_system::{
    job::{Job, LaunchMode, State, read_chunk},
    plan::Plan,
};
use std::{
    io,
    os::fd::OwnedFd,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::io::unix::AsyncFd;

pub(super) struct Running {
    id: i64,
    plan: Plan,
    mode: RunMode,
    pub job: Job,
    committed: bool,
    stdout: Capture,
    stderr: Capture,
    retained: AtomicUsize,
    limit: usize,
    modes: Option<rustix::termios::Termios>,
}
struct Capture {
    source: Option<AsyncFd<OwnedFd>>,
    bytes: Vec<u8>,
}
impl Capture {
    async fn drain(&mut self, limit: usize, retained: &AtomicUsize) -> io::Result<()> {
        while let Some(source) = &self.source {
            let chunk = read_chunk(source).await?;
            if chunk.is_empty() {
                self.source.take();
                break;
            }
            if retained
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                    used.checked_add(chunk.len())
                        .filter(|total| *total <= limit)
                })
                .is_err()
            {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "capture exceeds the byte limit",
                ));
            }
            self.bytes.extend(chunk);
        }
        Ok(())
    }
}
impl Running {
    pub fn reads_stdin(&self) -> bool {
        !self.captures()
            && self.plan.stages.first().is_some_and(|stage| {
                !stage
                    .redirects
                    .iter()
                    .any(|redirect| matches!(redirect, rill_system::plan::Redirect::Read(_)))
            })
    }

    pub const fn captures(&self) -> bool {
        matches!(self.mode, RunMode::Capture)
    }

    pub fn response(self) -> io::Result<Response> {
        let terminations = self
            .job
            .terminations()
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| io::Error::other("incomplete job result"))?;
        Ok(Response::Run {
            id: self.id,
            mode: self.mode,
            plan: self.plan,
            terminations,
            stdout: self.stdout.bytes,
            stderr: self.stderr.bytes,
            cancelled: false,
        })
    }

    pub async fn advance(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
    ) -> io::Result<()> {
        if !self.committed {
            if !matches!(self.mode, RunMode::Capture)
                && let Some(terminal) = terminal
            {
                terminal.handoff(
                    self.job
                        .group()
                        .ok_or_else(|| io::Error::other("missing process group"))?,
                )?;
            }
            self.job.commit().await?;
            self.committed = true;
        }
        tokio::try_join!(
            self.stdout.drain(self.limit, &self.retained),
            self.stderr.drain(self.limit, &self.retained),
            async {
                match self.job.wait().await? {
                    State::Finished => Ok(()),
                    State::Stopped => Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "foreground job stopped",
                    )),
                    State::Running => unreachable!("wait returns a settled state"),
                }
            }
        )
        .map(|_| ())
    }

    pub async fn suspend(&mut self, terminal: &rill_system::terminal::Terminal) -> io::Result<()> {
        self.job.signal(rustix::process::Signal::STOP)?;
        self.job.wait().await?;
        if !matches!(self.mode, RunMode::Capture) {
            self.modes = Some(terminal.modes()?);
        }
        Ok(())
    }
    pub fn resume(&mut self, terminal: &rill_system::terminal::Terminal) -> io::Result<()> {
        if !matches!(self.mode, RunMode::Capture) {
            terminal.handoff(
                self.job
                    .group()
                    .ok_or_else(|| io::Error::other("missing process group"))?,
            )?;
            if let Some(modes) = &self.modes {
                terminal.set_modes(modes)?;
            }
        }
        self.job.resume()
    }
}
impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "The coordinator owns each foreground run"
    )]
    pub(super) async fn run(
        &mut self,
        plan: Plan,
        mode: RunMode,
        max_bytes: usize,
    ) -> Result<Response, Error> {
        let run = self.prepare_run(plan, mode, max_bytes).await?;
        self.continue_run(run).await
    }

    #[expect(
        clippy::future_not_send,
        reason = "The GC coordinator owns foreground execution"
    )]
    pub(super) async fn prepare_run(
        &mut self,
        plan: Plan,
        mode: RunMode,
        max_bytes: usize,
    ) -> Result<Running, Error> {
        let capture = matches!(mode, RunMode::Capture);
        self.check_input_lease(&plan, capture)?;
        let id = self.jobs.allocate()?;
        let mut job = Job::prepare(
            &self.launcher,
            &plan,
            &self.snapshot,
            if capture {
                LaunchMode::Capture
            } else {
                LaunchMode::Foreground
            },
        )
        .await?;
        let stdout = Capture {
            source: job.stdout.take(),
            bytes: Vec::new(),
        };
        let stderr = Capture {
            source: job.stderr.take(),
            bytes: Vec::new(),
        };
        Ok(Running {
            id,
            plan,
            mode,
            job,
            stdout,
            stderr,
            committed: false,
            retained: AtomicUsize::new(0),
            limit: max_bytes,
            modes: None,
        })
    }
    #[expect(
        clippy::future_not_send,
        reason = "The GC coordinator owns foreground execution"
    )]
    pub(super) async fn continue_run(&mut self, mut run: Running) -> Result<Response, Error> {
        let capture = matches!(run.mode, RunMode::Capture);
        let execution = run.advance(self.terminal.as_ref());
        let result = tokio::select! {
            result = execution => result,
            _ = self.interrupt.recv(), if !self.engine.cleaning() => {
                self.interrupted = true;
                Err(io::Error::new(io::ErrorKind::Interrupted, "evaluation cancelled"))
            }
            () = super::stop_event(&mut self.stop), if self.terminal.is_some() && !self.engine.cleaning() => {
                self.suspend_requested = true;
                Err(io::Error::new(io::ErrorKind::WouldBlock, "evaluation stopped"))
            }
        };
        if self.terminal.is_some()
            && !self.engine.cleaning()
            && (self.suspend_requested
                || result
                    .as_ref()
                    .is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock))
        {
            self.suspend_requested = true;
            self.pending = Some(super::continuation::Pending::Run(Box::new(run)));
            return Ok(Response::Unit);
        }
        let restoration = self
            .terminal
            .as_ref()
            .map_or(Ok(()), rill_system::terminal::Terminal::restore);
        let result = result.and(restoration);
        match result {
            Ok(()) => {
                run.job.reap()?;
                if !capture && !self.engine.cleaning() && run.job.interrupted() {
                    self.interrupted = true;
                    return Err(Error::cancelled("foreground evaluation cancelled"));
                }
                run.response().map_err(Error::from)
            }
            Err(error) => {
                let mut error = Error::from_io(&error);
                if let Err(cleanup) = run.job.cancel().await {
                    error.notes.push(cleanup.to_string());
                }
                if self.interrupted
                    || (!capture && !self.engine.cleaning() && run.job.interrupted())
                {
                    self.interrupted = true;
                    let mut cancelled = Error::cancelled("foreground evaluation cancelled");
                    cancelled.notes = error.notes;
                    return Err(cancelled);
                }
                Err(error)
            }
        }
    }
}
