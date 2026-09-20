//! Process stdout is a source; terminal stderr uses an owned, cancellable output helper.
use crate::{
    job::{Job, State, read_chunk},
    plan::Plan,
    report,
    resources::SourceError,
};
use std::{io, os::fd::OwnedFd};
use tokio::io::unix::AsyncFd;

/// Own a process, its byte transports and optional terminal output relay until closure.
#[must_use = "process sources must be exhausted or explicitly closed"]
pub struct ProcessSource {
    pub job: Job,
    plan: Plan,
    output: Option<AsyncFd<OwnedFd>>,
    errors: Option<AsyncFd<OwnedFd>>,
    finished: bool,
    relay: Option<crate::writer::Writer>,
    relaying: bool,
}
impl ProcessSource {
    /// Transfer a committed job. Captured stderr requires an owned relay writer.
    pub const fn new(mut job: Job, plan: Plan, relay: Option<crate::writer::Writer>) -> Self {
        Self {
            output: job.stdout.take(),
            errors: job.stderr.take(),
            job,
            plan,
            finished: false,
            relay,
            relaying: false,
        }
    }
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, SourceError> {
        loop {
            if self.relaying {
                self.relay
                    .as_mut()
                    .expect("terminal stderr writer")
                    .flush()
                    .await?;
                self.relaying = false;
            }
            if self.output.is_none() && self.errors.is_none() && self.finished {
                self.job.reap()?;
                if let Some(relay) = self.relay.as_mut() {
                    relay.finish().await?;
                }
                self.relay = None;
                self.check()?;
                return Ok(None);
            }
            tokio::select! {
                result = read_optional(self.output.as_ref()), if self.output.is_some() => {
                    let bytes = result?;
                    if bytes.is_empty() { self.output.take(); }
                    else { return Ok(Some(bytes)); }
                }
                result = read_optional(self.errors.as_ref()), if self.errors.is_some() => {
                    let bytes = result?;
                    if bytes.is_empty() { self.errors.take(); }
                    else {
                        self.relay.as_mut().expect("terminal stderr writer").begin(bytes.into())?;
                        self.relaying = true;
                    }
                }
                state = self.job.wait(), if !self.finished => {
                    match state? {
                        State::Finished => self.finished = true,
                        State::Stopped => return Err(SourceError::Stopped),
                        State::Running => unreachable!("wait yields completion or stop"),
                    }
                }
            }
        }
    }
    pub async fn suspend(&mut self) -> io::Result<()> {
        self.job.signal(rustix::process::Signal::STOP)?;
        if let Some(relay) = self.relay.as_mut() {
            relay.suspend().await?;
        }
        self.job.wait().await.map(|_| ())
    }
    pub fn resume(&mut self) -> io::Result<()> {
        if let Some(relay) = self.relay.as_mut() {
            relay.resume()?;
        }
        self.job.resume()
    }
    fn check(&self) -> Result<(), SourceError> {
        let terminations = self
            .job
            .terminations()
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| io::Error::other("incomplete process result"))?;
        if let Some(index) = report::classify(&self.plan, &terminations).failure {
            return Err(SourceError::Process {
                stage: index,
                code: report::exit_code(terminations[index]),
            });
        }
        Ok(())
    }
    pub async fn close(mut self) -> Result<(), SourceError> {
        // Snapshot before closing pipes: new SIGPIPE can be caused by this cutoff,
        // but an already observed failure must keep its original meaning.
        let observation = self.job.poll().map_err(SourceError::from);
        let observed = self.job.terminations();
        self.output.take();
        self.errors.take();
        let cleanup = self.job.cancel().await.map_err(SourceError::from);
        let relay = if let Some(writer) = self.relay.as_mut() {
            writer.cancel().await
        } else {
            Ok(())
        };
        let result = cleanup
            .as_ref()
            .ok()
            .and_then(|()| self.check_cutoff(&observed).err());
        SourceError::combine(
            [
                observation.err(),
                result,
                relay.err().map(SourceError::from),
                cleanup.err(),
            ]
            .into_iter()
            .flatten(),
        )
    }
    fn check_cutoff(
        &self,
        observed: &[Option<crate::job::Termination>],
    ) -> Result<(), SourceError> {
        let terminations = self
            .job
            .terminations()
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| io::Error::other("incomplete process result"))?;
        let policy = report::classify_cutoff(&self.plan, &terminations, |index| {
            if observed[index].is_some() {
                return false;
            }
            let output_connected = !self.plan.stages[index].redirects.iter().any(|redirect| {
                matches!(
                    redirect,
                    crate::plan::Redirect::Write {
                        stream: crate::plan::Output::Stdout,
                        ..
                    }
                )
            }) && (index + 1 == self.plan.stages.len()
                || !self.plan.stages[index + 1]
                    .redirects
                    .iter()
                    .any(|redirect| matches!(redirect, crate::plan::Redirect::Read(_))));
            self.job.cleanup_termination(index)
                || (output_connected && crate::job::is_sigpipe(terminations[index]))
        });
        if let Some(index) = policy.failure {
            return Err(SourceError::Process {
                stage: index,
                code: report::exit_code(terminations[index]),
            });
        }
        Ok(())
    }
}
async fn read_optional(source: Option<&AsyncFd<OwnedFd>>) -> io::Result<Vec<u8>> {
    match source {
        Some(source) => read_chunk(source).await,
        None => std::future::pending().await,
    }
}
