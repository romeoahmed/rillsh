//! Lazily created output helpers belong to the evaluation, including when it is parked.
use super::{Session, continuation::Pending};
use rill_runtime::{Error, host::Response};
use rill_system::writer::Writer;
use std::io;

#[derive(Default)]
pub(super) struct Writers([Option<Writer>; 2]);
impl Writers {
    pub fn take(&mut self, stderr: bool) -> Option<Writer> {
        self.0[usize::from(stderr)].take()
    }
    pub async fn reuse(&mut self, stderr: bool, mut writer: Writer) -> io::Result<()> {
        let slot = &mut self.0[usize::from(stderr)];
        if slot.is_none() {
            *slot = Some(writer);
            Ok(())
        } else {
            writer.finish().await
        }
    }

    pub async fn cancel(&mut self, stderr: bool) -> io::Result<()> {
        match self.0[usize::from(stderr)].take() {
            Some(mut writer) => writer.cancel().await,
            None => Ok(()),
        }
    }
    pub async fn suspend(&mut self) -> io::Result<()> {
        for writer in self.0.iter_mut().flatten() {
            writer.suspend().await?;
        }
        Ok(())
    }
    pub fn resume(&mut self) -> io::Result<()> {
        for writer in self.0.iter_mut().flatten() {
            writer.resume()?;
        }
        Ok(())
    }
    pub async fn finish(&mut self) -> io::Result<()> {
        let mut result = Ok(());
        for mut writer in std::mem::take(&mut self.0).into_iter().flatten() {
            if let Err(error) = writer.finish().await {
                result = result.and(Err(error));
            }
        }
        result
    }
}
impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "Presentation belongs to the current-thread evaluation"
    )]
    pub(super) async fn display(&mut self, mut text: String) -> Result<Response, Error> {
        text.push('\n');
        self.output(text.into_bytes().into(), true).await
    }
    #[expect(
        clippy::future_not_send,
        reason = "The current-thread coordinator owns the evaluation and its output helpers"
    )]
    pub(super) async fn output(
        &mut self,
        bytes: bytes::Bytes,
        stderr: bool,
    ) -> Result<Response, Error> {
        let slot = &mut self.writers.0[usize::from(stderr)];
        if slot.is_none() {
            *slot = Some(Writer::prepare(&self.launcher, &self.snapshot, stderr).await?);
        }
        slot.as_mut().expect("prepared writer").begin(bytes)?;
        self.continue_output(stderr).await
    }
    #[expect(
        clippy::future_not_send,
        reason = "Output suspension retains a traced evaluation on the current-thread coordinator"
    )]
    pub(super) async fn continue_output(&mut self, stderr: bool) -> Result<Response, Error> {
        let writer = self.writers.0[usize::from(stderr)]
            .as_mut()
            .expect("pending output writer");
        let result = tokio::select! {
            result = writer.flush() => result.map_err(Error::from),
            _ = self.interrupt.recv(), if !self.engine.cleaning() => {
                self.interrupted = true;
                Err(Error::cancelled("output cancelled"))
            }
            () = super::stop_event(&mut self.stop), if self.terminal.is_some() && !self.engine.cleaning() => {
                self.suspend_requested = true;
                self.pending = Some(Pending::Output { stderr });
                return Ok(Response::Unit);
            }
        };
        match result {
            Ok(()) => Ok(Response::Unit),
            Err(mut error) => {
                if let Err(cleanup) = self.writers.cancel(stderr).await {
                    error.notes.push(cleanup.to_string());
                }
                Err(error)
            }
        }
    }
}
