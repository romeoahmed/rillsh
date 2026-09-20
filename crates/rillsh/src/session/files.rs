//! Owned file workers are joined before cancellation or terminal handoff completes.
use super::{Session, continuation, stop_event};
use rill_runtime::{Error, host::Response};
use std::{io, os::fd::OwnedFd, path::Path};

impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "Only owned Send file work leaves the current-thread coordinator; joined replies retain its continuation"
    )]
    pub(super) async fn file_work(
        &mut self,
        operation: impl FnOnce() -> io::Result<Response> + Send + 'static,
    ) -> Result<Response, Error> {
        let mut worker = tokio::task::spawn_blocking(operation);
        let result = tokio::select! {
            result = &mut worker => result,
            _ = self.interrupt.recv(), if !self.engine.cleaning() => {
                self.interrupted = true;
                worker.await
            }
            () = stop_event(&mut self.stop), if self.terminal.is_some() && !self.engine.cleaning() => {
                self.suspend_requested = true;
                worker.await
            }
        }.map_err(|error| Error::from_io(&io::Error::other(error)))?.map_err(Error::from);
        if self.suspend_requested {
            self.pending = Some(continuation::Pending::Reply(result));
            Ok(Response::Unit)
        } else {
            result
        }
    }
}

pub fn module(mut file: rill_system::source::SourceFile) -> io::Result<Response> {
    let text = file.read(16 * 1024 * 1024)?;
    Ok(Response::ModuleText { file, text })
}
pub fn text(cwd: &OwnedFd, path: &Path, max_bytes: usize) -> io::Result<Response> {
    rill_system::source::Directory::relative(cwd, Path::new("."))?
        .source(path)?
        .read(max_bytes)
        .map(Response::Text)
}
