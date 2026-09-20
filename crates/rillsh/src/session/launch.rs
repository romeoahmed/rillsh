//! A suspended launch retains its barrier and setup state; resumption never spawns twice.
use super::{Session, continuation::Pending, jobs::Entry};
use rill_runtime::{Error, host::Response};
use rill_system::{
    job::{Job, LaunchMode},
    plan::Plan,
};

pub(super) struct Launching {
    pub job: Job,
    plan: Plan,
    id: Option<i64>,
    relay: Option<rill_system::writer::Writer>,
}
impl Launching {
    pub async fn cancel(&mut self) -> std::io::Result<()> {
        let result = self.job.cancel().await;
        let relay = match self.relay.as_mut() {
            Some(relay) => relay.cancel().await,
            None => Ok(()),
        };
        match (result, relay) {
            (Err(error), Err(cleanup)) => Err(std::io::Error::new(
                error.kind(),
                format!("{error}; cleanup: {cleanup}"),
            )),
            (result, relay) => result.and(relay),
        }
    }
    pub async fn suspend(&mut self) -> std::io::Result<()> {
        self.job.signal(rustix::process::Signal::STOP)?;
        if let Some(relay) = self.relay.as_mut() {
            relay.suspend().await?;
        }
        self.job.wait().await.map(|_| ())
    }
    pub fn resume(&mut self) -> std::io::Result<()> {
        if let Some(relay) = self.relay.as_mut() {
            relay.resume()?;
        }
        self.job.resume()
    }
}
impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "The coordinator retains each launch until its barrier completes"
    )]
    pub(super) async fn launch(&mut self, plan: Plan, mode: LaunchMode) -> Result<Response, Error> {
        let launch = self.prepare_launch(plan, mode).await?;
        self.continue_launch(launch).await
    }
    pub(super) fn publish_launch(&mut self, mut launch: Launching) -> Response {
        if let Some(id) = launch.id {
            self.jobs
                .entries
                .insert(id, Entry::new(launch.job, launch.plan));
            return Response::Job(id);
        }
        let input = launch.job.stdin.take().map(|fd| self.sources.output(fd));
        let output = self.sources.process(launch.job, launch.plan, launch.relay);
        input.map_or(Response::Source(output), |input| Response::Through {
            output,
            input,
        })
    }

    #[expect(
        clippy::future_not_send,
        reason = "Launch ownership belongs to the coordinator"
    )]
    pub(super) async fn prepare_launch(
        &mut self,
        plan: Plan,
        mode: LaunchMode,
    ) -> Result<Launching, Error> {
        let id = if mode == LaunchMode::Background {
            Some(self.jobs.allocate()?)
        } else {
            None
        };
        let job = Job::prepare(&self.launcher, &plan, &self.snapshot, mode).await?;
        let mut launch = Launching {
            job,
            plan,
            id,
            relay: None,
        };
        if launch.job.stderr.is_some() {
            match rill_system::writer::Writer::prepare(&self.launcher, &self.snapshot, true).await {
                Ok(writer) => launch.relay = Some(writer),
                Err(error) => {
                    let mut error = Error::from_io(&error);
                    if let Err(cleanup) = launch.cancel().await {
                        error.notes.push(cleanup.to_string());
                    }
                    return Err(error);
                }
            }
        }
        Ok(launch)
    }
    #[expect(
        clippy::future_not_send,
        reason = "A stopped launch remains owned until resumed or cancelled"
    )]
    pub(super) async fn continue_launch(
        &mut self,
        mut launch: Launching,
    ) -> Result<Response, Error> {
        let result = tokio::select! {
            result = launch.job.commit() => result,
            _ = self.interrupt.recv(), if !self.engine.cleaning() => {
                self.interrupted = true;
                Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "launch cancelled"))
            }
            () = super::stop_event(&mut self.stop), if self.terminal.is_some() && !self.engine.cleaning() => {
                self.suspend_requested = true;
                Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "launch stopped"))
            }
        };
        if self.terminal.is_some()
            && !self.engine.cleaning()
            && (self.suspend_requested
                || result
                    .as_ref()
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock))
        {
            self.suspend_requested = true;
            self.pending = Some(Pending::Launch(Box::new(launch)));
            return Ok(Response::Unit);
        }
        if let Err(error) = result {
            let mut error = if self.interrupted {
                Error::cancelled("launch cancelled")
            } else {
                Error::from_io(&error)
            };
            if let Err(cleanup) = launch.cancel().await {
                error.notes.push(cleanup.to_string());
            }
            return Err(error);
        }
        Ok(self.publish_launch(launch))
    }
}
