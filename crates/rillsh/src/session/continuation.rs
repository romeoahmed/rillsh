//! Parked evaluations retain VM roots and their own OS resources until fg or cancellation.
use super::{Session, execution::Running};
use rill_runtime::{
    Error,
    host::{Control, Response},
};
use rill_system::resources::{SourceError, SourceId, Sources};

pub(super) enum Pending {
    Reply(Result<Response, Error>),
    Output { stderr: bool },
    Run(Box<Running>),
    Glob(Box<Running>),
    Launch(Box<super::launch::Launching>),
    Control { id: i64, operation: Control },
    Read { keys: Vec<SourceId>, wait: bool },
}
#[derive(Default)]
pub(super) struct Suspended {
    sources: Sources,
    writers: super::output::Writers,
    operations: super::operations::Operations,
    pending: Option<Pending>,
    resumers: Vec<i64>,
}
impl Suspended {
    pub(super) fn foreground_job(&self) -> Option<i64> {
        if let Some(id) = self.operations.foreground_job() {
            return Some(id);
        }
        match self.pending {
            Some(Pending::Control {
                id,
                operation: Control::Foreground,
            }) => Some(id),
            _ => None,
        }
    }
}
impl Session {
    pub(super) fn retained_caller(&self, id: i64) -> bool {
        self.active_evaluations.contains(&id)
            || self.resumers.contains(&id)
            || self
                .suspended
                .values()
                .any(|entry| entry.resumers.contains(&id))
    }
    pub(super) fn stdin_leased(&self) -> bool {
        self.sources.stdin_leased()
            || self
                .suspended
                .values()
                .any(|entry| entry.sources.stdin_leased())
    }
    #[expect(
        clippy::future_not_send,
        reason = "The coordinator owns suspended source reads"
    )]
    pub(super) async fn read_sources(
        &mut self,
        keys: Vec<SourceId>,
        wait: bool,
    ) -> Result<Response, Error> {
        loop {
            let result = tokio::select! {
                result = super::operations::readiness(&mut self.sources, &mut self.operations, &keys, wait, self.terminal.as_ref(), &mut self.jobs) => result,
                _ = self.child_events.recv() => continue,
                _ = self.interrupt.recv(), if !self.engine.cleaning() => {
                    self.interrupted = true;
                    return Err(Error::cancelled("evaluation cancelled"));
                }
                () = super::stop_event(&mut self.stop), if self.terminal.is_some() && !self.engine.cleaning() => {
                    super::operations::Ready::Source(Err(SourceError::Stopped))
                }
            };
            let result = match result {
                super::operations::Ready::Operation(index, key, result) => {
                    if result
                        .as_ref()
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock)
                        && self.terminal.is_some()
                        && !self.engine.cleaning()
                    {
                        self.pending = Some(Pending::Read { keys, wait });
                        self.suspend_requested = true;
                        return Ok(Response::Unit);
                    }
                    let result = self.complete_operation(key, result).await;
                    if let Some(index) = index {
                        self.sources.close(key).await?;
                        return Ok(Response::Operation {
                            index,
                            result: result.map(Box::new),
                        });
                    }
                    if self.interrupted {
                        self.sources.close(key).await?;
                        return Err(Error::cancelled("foreground evaluation cancelled"));
                    }
                    self.retain_operation(key, result);
                    continue;
                }
                super::operations::Ready::Source(result) => result,
            };
            if (matches!(result, Err(SourceError::Stopped))
                || matches!(&result, Err(SourceError::Selected { error, .. }) if matches!(**error, SourceError::Stopped)))
                && self.terminal.is_some()
                && !self.engine.cleaning()
            {
                self.pending = Some(Pending::Read { keys, wait });
                self.suspend_requested = true;
                return Ok(Response::Unit);
            }
            let result = match result {
                Err(SourceError::Selected { index, error }) => {
                    return Ok(Response::ReadFailure { index, error });
                }
                result => result,
            };
            return result
                .map(|ready| {
                    ready.map_or(Response::Pending, |(index, value)| Response::Item {
                        index,
                        value,
                    })
                })
                .map_err(Error::from);
        }
    }

    fn retain(&mut self, id: i64) -> Result<(), Error> {
        self.engine.suspend(id)?;
        self.suspended.insert(
            id,
            Suspended {
                sources: std::mem::take(&mut self.sources),
                writers: std::mem::take(&mut self.writers),
                operations: std::mem::take(&mut self.operations),
                pending: self.pending.take(),
                resumers: std::mem::take(&mut self.resumers),
            },
        );
        Ok(())
    }
    fn restore(&mut self, id: i64) -> Result<(), Error> {
        self.engine.activate(id)?;
        let entry = self
            .suspended
            .remove(&id)
            .expect("parked VM owns its host continuation");
        self.sources = entry.sources;
        self.writers = entry.writers;
        self.operations = entry.operations;
        self.pending = entry.pending;
        self.resumers = entry.resumers;
        Ok(())
    }
    #[expect(
        clippy::future_not_send,
        reason = "A stop barrier precedes returning control to the editor"
    )]
    pub(super) async fn park(&mut self) -> Result<(), Error> {
        self.suspend_requested = false;
        if let Err(mut error) = self.freeze_active().await {
            if let Err(cleanup) = self.cancel_pending().await {
                error.notes.push(cleanup.to_string());
            }
            if let Err(cleanup) = self.sources.resume() {
                error.notes.push(cleanup.to_string());
            }
            self.engine.fail(error)?;
            return Box::pin(self.drive_current()).await;
        }
        let id = match self.active_evaluations.last() {
            Some(id) => *id,
            None => self.jobs.allocate()?,
        };
        self.retain(id)?;
        self.terminal()
            .write(format!("\r\n[job {id}] stopped\r\n").as_bytes())
            .map_err(Error::from)
    }
    #[expect(
        clippy::future_not_send,
        reason = "Freeze and observe every attached group before terminal handoff"
    )]
    async fn freeze_active(&mut self) -> Result<(), Error> {
        self.sources.suspend().await?;
        self.writers.suspend().await?;
        self.operations
            .suspend(self.terminal.as_ref(), &mut self.jobs)
            .await?;
        if let Some(Pending::Run(run) | Pending::Glob(run)) = &mut self.pending {
            run.suspend(self.terminal.as_ref().expect("interactive suspension"))
                .await?;
        }
        if let Some(Pending::Launch(launch)) = &mut self.pending {
            launch.suspend().await?;
        }
        if let Some(Pending::Control {
            id,
            operation: Control::Foreground,
        }) = &self.pending
        {
            let entry = self.jobs.get(*id)?;
            entry.job.signal(rustix::process::Signal::STOP)?;
            entry.job.wait().await?;
            entry.modes = Some(
                self.terminal
                    .as_ref()
                    .expect("interactive suspension")
                    .modes()?,
            );
        }
        self.terminal().restore()?;
        Ok(())
    }
    #[expect(
        clippy::future_not_send,
        reason = "Only one language continuation runs on the coordinator"
    )]
    pub(super) async fn control_evaluation(
        &mut self,
        id: i64,
        operation: Control,
    ) -> Result<(), Error> {
        if self.retained_caller(id) {
            return self.engine.resume(Err(Error::new(
                "JobError",
                "cannot control the calling evaluation",
            )));
        }
        if matches!(operation, Control::Wait | Control::Background) {
            return self.engine.resume(Err(Error::new(
                "JobError",
                "a suspended evaluation requires fg or cancel",
            )));
        }
        let caller = self
            .active_evaluations
            .last()
            .copied()
            .map_or_else(|| self.jobs.allocate(), Ok)?;
        self.freeze_active().await?;
        self.retain(caller)?;
        self.active_evaluations.push(caller);
        self.active_evaluations.push(id);
        self.restore(id)?;
        self.engine.consume_result();
        self.interrupted = false;
        let cancel = matches!(operation, Control::Cancel);
        let result = self.run_retained(cancel).await;
        self.active_evaluations.pop();
        self.active_evaluations.pop();
        if self.suspended.contains_key(&id) {
            // A second stop retains the fg caller too. Resumption must continue its
            // expression exactly once, not turn the stop into a catchable error.
            self.suspended
                .get_mut(&id)
                .expect("stopped callee")
                .resumers
                .insert(0, caller);
            return Ok(());
        }
        let saved = self
            .suspended
            .remove(&caller)
            .expect("foreground caller remains retained");
        self.sources = saved.sources;
        self.writers = saved.writers;
        self.operations = saved.operations;
        self.pending = saved.pending;
        self.resumers = saved.resumers;
        self.interrupted = false;
        let result = match (result, self.resume_owned()) {
            (Ok(()), result) => result,
            (Err(mut error), resumed) => {
                if let Err(cleanup) = resumed {
                    error.notes.push(cleanup.to_string());
                }
                Err(error)
            }
        };
        self.engine.return_to(caller, result)
    }
    #[expect(
        clippy::future_not_send,
        reason = "Resume and cancellation keep explicit resources on the coordinator"
    )]
    async fn run_retained(&mut self, cancel: bool) -> Result<(), Error> {
        let mut setup = Ok(());
        let mut failures = Vec::new();
        loop {
            let result = match setup {
                Ok(()) => self.run_segment(cancel).await,
                Err(error) => self.finish_resources(Err(error)).await,
            };
            let result = if cancel {
                if let Err(error) = result {
                    failures.push(error);
                }
                Ok(())
            } else {
                result
            };
            let Some(caller) = self.resumers.pop() else {
                let mut failures = failures.into_iter();
                if let Some(mut error) = failures.next() {
                    error.notes.extend(failures.map(|error| error.to_string()));
                    return Err(error);
                }
                return result;
            };
            let parent = self
                .suspended
                .remove(&caller)
                .expect("retained foreground chain");
            self.sources = parent.sources;
            self.writers = parent.writers;
            self.operations = parent.operations;
            self.pending = parent.pending;
            self.resumers.extend(parent.resumers);
            setup = if cancel {
                self.engine.activate(caller)
            } else {
                self.engine.return_to(caller, result)
            };
            self.engine.consume_result();
        }
    }
    #[expect(
        clippy::future_not_send,
        reason = "Owned foreground jobs must be reaped before discarding a continuation"
    )]
    async fn cancel_pending(&mut self) -> Result<(), Error> {
        match self.pending.take() {
            Some(Pending::Output { stderr }) => {
                self.writers.cancel(stderr).await.map_err(Error::from)
            }
            Some(Pending::Run(mut run) | Pending::Glob(mut run)) => {
                run.job.cancel().await.map_err(Error::from)
            }
            Some(Pending::Launch(mut launch)) => launch.cancel().await.map_err(Error::from),
            Some(Pending::Control {
                id,
                operation: Control::Foreground,
            }) => {
                let entry = self.jobs.get(id)?;
                entry.cancelled = true;
                entry.acknowledged = true;
                entry.job.cancel().await.map_err(Error::from)
            }
            _ => Ok(()),
        }
    }
    fn resume_owned(&mut self) -> Result<(), Error> {
        self.sources.resume()?;
        self.writers.resume()?;
        self.operations
            .resume(self.terminal.as_ref(), &mut self.jobs)?;
        if let Some(Pending::Launch(launch)) = &mut self.pending {
            launch.resume()?;
        }
        if let Some(Pending::Run(run) | Pending::Glob(run)) = &mut self.pending {
            run.resume(self.terminal.as_ref().expect("interactive resumption"))?;
        }
        Ok(())
    }
    #[expect(
        clippy::future_not_send,
        reason = "Each foreground segment owns its cleanup"
    )]
    async fn run_segment(&mut self, cancel: bool) -> Result<(), Error> {
        let mut cleanup_error = None;
        let setup = if cancel {
            cleanup_error = self.cancel_pending().await.err();
            self.engine.interrupt()
        } else {
            match self.resume_owned() {
                Ok(()) => Ok(()),
                Err(mut error) => {
                    if let Err(cleanup) = self.cancel_pending().await {
                        error.notes.push(cleanup.to_string());
                    }
                    self.engine.fail(error)
                }
            }
        };
        let result = match setup {
            Ok(()) => Box::pin(self.drive_current()).await,
            Err(error) => Err(error),
        };
        let result = match result {
            Err(error) if cancel && error.is_cancelled() => {
                if error.notes.is_empty() {
                    Ok(())
                } else {
                    // Explicit cancel ends the target, not its caller. Cleanup failure
                    // remains an ordinary error the caller can handle with attempt.
                    let mut cleanup =
                        Error::new("CleanupError", "cancelled evaluation cleanup failed");
                    cleanup.notes = error.notes;
                    Err(cleanup)
                }
            }
            result => result,
        };
        let result = self.finish_resources(result).await;
        match (result, cleanup_error) {
            (Ok(()), Some(error)) => Err(error),
            (Err(mut error), Some(cleanup)) => {
                error.notes.push(cleanup.to_string());
                Err(error)
            }
            (result, None) => result,
        }
    }
    #[expect(
        clippy::future_not_send,
        reason = "All resources are joined before returning to a caller"
    )]
    pub(super) async fn finish_resources(
        &mut self,
        result: Result<(), Error>,
    ) -> Result<(), Error> {
        let keys = self
            .operations
            .close_all(self.terminal.as_ref(), &mut self.jobs)
            .await
            .map_err(Error::from);
        let mut cleanup = self.sources.close_all().await.map_err(Error::from);
        if let Err(error) = keys {
            match &mut cleanup {
                Ok(()) => cleanup = Err(error),
                Err(primary) => primary.notes.push(error.to_string()),
            }
        }
        if let Err(error) = self.writers.finish().await {
            match &mut cleanup {
                Ok(()) => cleanup = Err(Error::from_io(&error)),
                Err(primary) => primary.notes.push(error.to_string()),
            }
        }
        if let Some(terminal) = &self.terminal
            && let Err(error) = terminal.restore()
        {
            match &mut cleanup {
                Ok(()) => cleanup = Err(Error::from_io(&error)),
                Err(primary) => primary.notes.push(error.to_string()),
            }
        }
        match (result, cleanup) {
            (Ok(()), cleanup) => cleanup,
            (Err(mut error), cleanup) => {
                if let Err(cleanup) = cleanup {
                    error.notes.push(cleanup.to_string());
                }
                Err(error)
            }
        }
    }
    #[expect(
        clippy::future_not_send,
        reason = "Shutdown runs each retained release continuation to completion"
    )]
    pub(super) async fn shutdown_evaluations(&mut self) -> Result<(), Error> {
        let mut errors = Vec::new();
        while let Some(id) = self
            .suspended
            .keys()
            .copied()
            .find(|id| !self.retained_caller(*id))
        {
            self.restore(id)?;
            if let Err(error) = self.run_retained(true).await {
                errors.push(error);
            }
        }
        let mut errors = errors.into_iter();
        if let Some(mut error) = errors.next() {
            error.notes.extend(errors.map(|error| error.to_string()));
            Err(error)
        } else {
            Ok(())
        }
    }
}
