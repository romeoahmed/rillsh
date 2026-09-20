//! Cancellable descriptor reads preserve shared status flags and join blocking workers.
use rustix::{
    event::{PollFd, PollFlags, poll},
    io::{Errno, read},
};
use std::{io, os::fd::OwnedFd};
use tokio::task::JoinHandle;

type ReadResult = (OwnedFd, OwnedFd, io::Result<Option<Vec<u8>>>);

pub struct Input {
    idle: Option<(OwnedFd, OwnedFd)>,
    worker: Option<JoinHandle<ReadResult>>,
    cancel: Option<OwnedFd>,
    pending: Option<io::Result<Vec<u8>>>,
}
impl Input {
    /// Own a descriptor and create its cancellation channel.
    ///
    /// # Errors
    /// Reports cancellation-pipe creation failure.
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        let (cancel_read, cancel_write) = crate::sys::pipe()?;
        Ok(Self {
            idle: Some((fd, cancel_read)),
            worker: None,
            cancel: Some(cancel_write),
            pending: None,
        })
    }
    /// Read one chunk, retaining an in-flight worker if this future is dropped.
    ///
    /// # Errors
    /// Reports polling, reading or worker failures.
    pub async fn next(&mut self) -> io::Result<Vec<u8>> {
        if let Some(result) = self.pending.take() {
            return result;
        }
        if let Some((fd, cancel)) = self.idle.take() {
            self.worker = Some(tokio::task::spawn_blocking(move || {
                let result = read_ready(&fd, &cancel);
                (fd, cancel, result)
            }));
        }
        let result = self
            .worker
            .as_mut()
            .ok_or_else(|| io::Error::other("input reader is closed"))?
            .await;
        self.worker.take();
        let (fd, cancel, result) = result.map_err(io::Error::other)?;
        self.idle = Some((fd, cancel));
        result.map(Option::unwrap_or_default)
    }
    /// Stop a pending reader before terminal editing resumes, retaining any completed chunk.
    /// The lease and shared descriptor flags remain unchanged.
    ///
    /// # Errors
    /// Reports cancellation-channel or worker failures.
    pub async fn pause(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            self.cancel.take();
            let (fd, _, result) = worker.await.map_err(io::Error::other)?;
            self.pending = result.transpose();
            let (read, write) = crate::sys::pipe()?;
            self.idle = Some((fd, read));
            self.cancel = Some(write);
        }
        Ok(())
    }
    /// Cancel and join any pending read before releasing descriptor ownership.
    ///
    /// # Errors
    /// Reports a completed read or worker failure.
    pub async fn close(mut self) -> io::Result<()> {
        self.cancel.take();
        if let Some(result) = self.pending.take() {
            result?;
        }
        if let Some(worker) = self.worker.take() {
            let (_, _, result) = worker.await.map_err(io::Error::other)?;
            result?;
        }
        Ok(())
    }
}
fn read_ready(fd: &OwnedFd, cancel: &OwnedFd) -> io::Result<Option<Vec<u8>>> {
    let mut events = [
        PollFd::new(fd, PollFlags::IN),
        PollFd::new(cancel, PollFlags::IN),
    ];
    loop {
        match poll(&mut events, None) {
            Err(Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        if !events[1].revents().is_empty() {
            return Ok(None);
        }
        if events[0].revents().is_empty() {
            continue;
        }
        let mut bytes = vec![0; crate::IO_CHUNK_BYTES];
        match read(fd, &mut bytes) {
            Ok(count) => {
                bytes.truncate(count);
                return Ok(Some(bytes));
            }
            Err(Errno::INTR | Errno::AGAIN) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Input;
    use rustix::fs::fcntl_getfl;

    #[tokio::test(flavor = "current_thread")]
    async fn cancelling_a_pending_read_joins_the_worker_without_changing_shared_flags() {
        let (reader, _writer) = crate::sys::pipe().unwrap();
        let observer = reader.try_clone().unwrap();
        let flags = fcntl_getfl(&observer).unwrap();
        let mut input = Input::new(reader).unwrap();
        {
            let next = input.next();
            tokio::pin!(next);
            tokio::select! {
                biased;
                result = &mut next => panic!("empty open pipe unexpectedly completed: {result:?}"),
                () = tokio::task::yield_now() => {}
            }
        }
        input.close().await.unwrap();
        assert_eq!(fcntl_getfl(&observer).unwrap(), flags);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pausing_a_read_keeps_every_byte_and_eof_available_after_resumption() {
        let (reader, writer) = crate::sys::pipe().unwrap();
        let mut input = Input::new(reader).unwrap();
        {
            let read = input.next();
            tokio::pin!(read);
            // Register a read while the open pipe is empty, then stop awaiting it.
            std::future::poll_fn(|cx| {
                use std::{future::Future, task::Poll};
                assert!(read.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        rustix::io::write(&writer, b"retained input").unwrap();
        drop(writer);
        // Cancellation may win the race or the worker may already hold the bytes.
        input.pause().await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut bytes = Vec::new();
            loop {
                let chunk = input.next().await.unwrap();
                if chunk.is_empty() {
                    break;
                }
                bytes.extend(chunk);
            }
            bytes
        })
        .await;
        input.close().await.unwrap();
        assert_eq!(result.unwrap(), b"retained input");
    }
}
