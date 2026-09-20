//! Session job identities outlive process resources and retain immutable completion data.
use rill_runtime::{
    Error,
    host::{JobSnapshot, Response, RunMode},
};
use rill_system::{
    job::{Job, State},
    plan::Plan,
};
use std::collections::BTreeMap;

pub struct Entry {
    pub job: Job,
    pub plan: Plan,
    pub cancelled: bool,
    pub acknowledged: bool,
    notified: State,
    pub modes: Option<rustix::termios::Termios>,
}
#[derive(Default)]
pub struct Jobs {
    next: i64,
    pub entries: BTreeMap<i64, Entry>,
}
impl Jobs {
    pub fn allocate(&mut self) -> Result<i64, Error> {
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| Error::new("LimitExceeded", "job identities exhausted"))?;
        Ok(self.next)
    }
    pub fn notify(&mut self, output: &rill_editor::Notifications) -> Result<bool, Error> {
        for (id, entry) in &mut self.entries {
            let state = entry.poll()?;
            if !entry.acknowledged && state != entry.notified {
                let message = match state {
                    State::Running => "running",
                    State::Stopped => "stopped",
                    State::Finished => "completed",
                };
                if !output.try_print(format!("[job {id}] {message}")) {
                    return Ok(true);
                }
            }
            entry.notified = state;
        }
        Ok(false)
    }
    pub fn get(&mut self, id: i64) -> Result<&mut Entry, Error> {
        self.entries
            .get_mut(&id)
            .ok_or_else(|| Error::new("JobError", "job does not belong to this session"))
    }
    pub fn snapshots(&mut self) -> Result<Vec<JobSnapshot>, Error> {
        self.entries
            .iter_mut()
            .map(|(id, entry)| {
                let state = entry.poll()?;
                Ok(JobSnapshot {
                    id: *id,
                    kind: "process",
                    state: match state {
                        State::Running => "running",
                        State::Stopped => "stopped",
                        State::Finished => "completed",
                    },
                })
            })
            .collect()
    }
    pub fn live(&mut self) -> Result<bool, Error> {
        let mut live = false;
        for entry in self.entries.values_mut() {
            // Visit every entry so completed peers are reaped even while another runs.
            live |= entry.poll()? != State::Finished;
        }
        Ok(live)
    }
    pub fn unacknowledged(&self) -> bool {
        self.entries.values().any(|entry| !entry.acknowledged)
    }
    pub async fn shutdown(&mut self) -> Result<(), Error> {
        let mut errors = Vec::new();
        for entry in self.entries.values_mut() {
            if let Err(error) = entry.job.cancel().await {
                errors.push(error);
            }
        }
        let mut errors = errors.into_iter();
        let Some(first) = errors.next() else {
            return Ok(());
        };
        let mut error = Error::from_io(&first);
        error.notes.extend(errors.map(|error| error.to_string()));
        Err(error)
    }
}
impl Entry {
    fn poll(&mut self) -> Result<State, Error> {
        let state = self.job.poll()?;
        if state == State::Finished {
            self.job.reap()?;
        }
        Ok(state)
    }

    pub(super) const fn new(job: Job, plan: Plan) -> Self {
        Self {
            job,
            plan,
            cancelled: false,
            acknowledged: false,
            notified: State::Running,
            modes: None,
        }
    }

    pub(super) fn foreground(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
    ) -> Result<(), Error> {
        self.foreground_native(terminal).map_err(Error::from)
    }
    pub(super) fn foreground_native(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
    ) -> std::io::Result<()> {
        let result = (|| {
            if let Some(terminal) = terminal {
                terminal.handoff(
                    self.job
                        .group()
                        .ok_or_else(|| std::io::Error::other("job has no process group"))?,
                )?;
                if let Some(modes) = &self.modes {
                    terminal.set_modes(modes)?;
                }
            }
            self.job.resume()
        })();
        if result.is_err()
            && let Some(terminal) = terminal
        {
            let _ = terminal.restore();
        }
        result
    }

    pub fn response(&mut self, id: i64, mode: RunMode) -> Result<Response, Error> {
        self.native_response(id, mode).map_err(Error::from)
    }
    pub(super) fn native_response(&mut self, id: i64, mode: RunMode) -> std::io::Result<Response> {
        let terminations = self
            .job
            .terminations()
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| std::io::Error::other("job has not completed"))?;
        self.job.reap()?;
        self.acknowledged = true;
        Ok(Response::Run {
            id,
            mode,
            plan: self.plan.clone(),
            terminations,
            stdout: Vec::new(),
            stderr: Vec::new(),
            cancelled: self.cancelled,
        })
    }
}

impl super::Session {
    pub(super) fn job_snapshots(&mut self) -> Result<Vec<JobSnapshot>, Error> {
        let mut jobs = self.jobs.snapshots()?;
        jobs.extend(
            self.suspended
                .keys()
                .filter(|id| !self.retained_caller(**id))
                .map(|id| JobSnapshot {
                    id: *id,
                    kind: "evaluation",
                    state: "stopped",
                }),
        );
        jobs.sort_unstable_by_key(|job| job.id);
        Ok(jobs)
    }

    #[expect(
        clippy::future_not_send,
        reason = "The GC coordinator and its job table belong to the current-thread runtime"
    )]
    pub(super) async fn control(
        &mut self,
        id: i64,
        operation: rill_runtime::host::Control,
    ) -> Result<Response, Error> {
        use rill_runtime::host::Control;
        if self.active_evaluations.contains(&id) {
            return Err(Error::new(
                "JobError",
                "cannot control the calling evaluation",
            ));
        }
        if self
            .suspended
            .values()
            .any(|entry| entry.foreground_job() == Some(id))
        {
            return Err(Error::new(
                "JobError",
                "job belongs to a suspended evaluation; foreground that evaluation instead",
            ));
        }
        let entry = self.jobs.get(id)?;
        let state = entry.job.poll()?;
        match operation {
            Control::Cancel => {
                if state != State::Finished {
                    entry.cancelled = true;
                    entry.job.cancel().await?;
                }
                entry.acknowledged = true;
                return Ok(Response::Unit);
            }
            Control::Background => {
                if state == State::Stopped {
                    entry.job.resume()?;
                }
                return Ok(Response::Unit);
            }
            Control::Wait | Control::Foreground => {}
        }
        let foreground = matches!(operation, Control::Foreground);
        if foreground && state != State::Finished {
            entry.foreground(self.terminal.as_ref())?;
        }

        let result = tokio::select! {
            result = entry.job.wait() => result.map_err(Error::from),
            _ = self.interrupt.recv(), if !self.engine.cleaning() => {
                self.interrupted = true;
                Err(Error::cancelled("job wait cancelled"))
            }
            () = super::stop_event(&mut self.stop), if self.terminal.is_some() && !self.engine.cleaning() => {
                self.suspend_requested = true;
                Ok(State::Stopped)
            }
        };
        if self.terminal.is_some()
            && !self.engine.cleaning()
            && (self.suspend_requested || matches!(result, Ok(State::Stopped)) && foreground)
        {
            self.suspend_requested = true;
            self.pending = Some(super::continuation::Pending::Control { id, operation });
            return Ok(Response::Unit);
        }
        let restoration = if foreground {
            self.terminal
                .as_ref()
                .map_or(Ok(()), rill_system::terminal::Terminal::restore)
        } else {
            Ok(())
        };
        if self.interrupted && foreground {
            entry.cancelled = true;
            entry.acknowledged = true;
            entry.job.cancel().await?;
        }
        let state = result?;
        restoration?;
        if state == State::Stopped {
            return Err(Error::new(
                "JobStopped",
                "job is stopped; use bg or fg to resume it",
            ));
        }
        if foreground && !self.engine.cleaning() && entry.job.interrupted() {
            self.interrupted = true;
            entry.acknowledged = true;
            entry.job.reap()?;
            return Err(Error::cancelled("foreground evaluation cancelled"));
        }
        entry.response(
            id,
            if foreground {
                RunMode::Checked
            } else {
                RunMode::Report
            },
        )
    }
    pub fn can_exit(&mut self) -> Result<(), Error> {
        if self.jobs.live()? || !self.suspended.is_empty() {
            Err(Error::new(
                "JobsActive",
                "jobs are still running or stopped; finish them or use exit_force",
            ))
        } else {
            Ok(())
        }
    }
    #[expect(
        clippy::future_not_send,
        reason = "The GC coordinator owns session shutdown on the current-thread runtime"
    )]
    pub async fn shutdown(&mut self, script: bool) -> Result<(), Error> {
        let unacknowledged = script && self.exit.is_none() && self.jobs.unacknowledged();
        let evaluations = self.shutdown_evaluations().await;
        let cleanup = self.jobs.shutdown().await.and(evaluations);
        if unacknowledged {
            let mut error = Error::new(
                "JobsActive",
                "script ended with unacknowledged jobs; wait, foreground, or cancel them explicitly",
            );
            if let Err(cleanup) = cleanup {
                error.notes.push(cleanup.to_string());
            }
            return Err(error);
        }
        cleanup
    }
}
