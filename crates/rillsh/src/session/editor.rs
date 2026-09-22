//! Modal buffer editing uses the ordinary launcher and explicit terminal ownership.
use super::Session;
use rill_runtime::Error;
use rill_system::{
    job::{Job, LaunchMode, State, Termination},
    plan::{Plan, Stage},
};
use std::{ffi::CString, os::unix::ffi::OsStrExt};

impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "The session and terminal remain on the coordinator thread"
    )]
    pub async fn edit_buffer(&mut self, source: &str) -> Result<String, Error> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("input.rill");
        std::fs::write(&path, source)?;
        let executable = self
            .snapshot
            .environment
            .get(c"VISUAL")
            .or_else(|| self.snapshot.environment.get(c"EDITOR"))
            .cloned()
            .unwrap_or_else(|| c"vi".into());
        let plan = Plan {
            stages: vec![Stage {
                argv: vec![
                    executable,
                    CString::new(path.as_os_str().as_bytes())
                        .map_err(|_| Error::new("IOError", "editor path contains NUL"))?,
                ],
                ..Stage::default()
            }],
        };
        self.terminal().restore()?;
        self.terminal().finish_line()?;
        let mut job = Job::prepare(
            &self.launcher,
            &plan,
            &self.snapshot,
            LaunchMode::Foreground,
        )
        .await?;
        let result = self.edit_job(&mut job).await;
        let cleanup = job.cancel().await;
        let restored = self.terminal().restore();
        let mut error = result.err();
        for failure in [cleanup, restored].into_iter().filter_map(Result::err) {
            if let Some(primary) = &mut error {
                primary.notes.push(failure.to_string());
            } else {
                error = Some(failure.into());
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        tokio::task::spawn_blocking(move || {
            // Editors may replace the file atomically; reopen its name after successful exit.
            rill_system::source::Directory::open(directory.path())?
                .source(std::path::Path::new("input.rill"))?
                .read(1024 * 1024)
        })
        .await
        .map_err(|error| Error::new("IOError", error.to_string()))?
        .map_err(Error::from)
    }
    #[expect(
        clippy::future_not_send,
        reason = "The modal editor borrows the session's terminal and signals"
    )]
    async fn edit_job(&mut self, job: &mut Job) -> Result<(), Error> {
        let group = job
            .group()
            .ok_or_else(|| Error::new("JobError", "editor has no process group"))?;
        self.terminal().handoff(group)?;
        job.commit().await?;
        loop {
            let state = tokio::select! {
                result = job.wait() => result?,
                _ = self.interrupt.recv() => return Err(Error::cancelled("editing cancelled")),
                () = super::stop_event(&mut self.stop) => {
                    job.signal(rustix::process::Signal::STOP)?;
                    job.wait().await?
                }
            };
            if state == State::Finished {
                if job.interrupted() {
                    return Err(Error::cancelled("editing cancelled"));
                }
                return if job.terminations() == [Some(Termination::Exited(0))] {
                    Ok(())
                } else {
                    Err(Error::new(
                        "EditorError",
                        "editor did not finish successfully; original input retained",
                    ))
                };
            }
            let modes = self.terminal().modes()?;
            self.terminal().suspend()?;
            self.terminal().handoff(group)?;
            self.terminal().set_modes(&modes)?;
            job.resume()?;
        }
    }
}
