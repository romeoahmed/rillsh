//! One bounded transport chunk, advanced without blocking the reactor or losing partial writes.
use std::{io, os::fd::OwnedFd};
use tokio::io::unix::AsyncFd;

pub struct Output {
    fd: Option<AsyncFd<OwnedFd>>,
    bytes: bytes::Bytes,
    offset: usize,
}
impl Output {
    pub const fn new(fd: AsyncFd<OwnedFd>) -> Self {
        Self {
            fd: Some(fd),
            bytes: bytes::Bytes::new(),
            offset: 0,
        }
    }
    pub fn enqueue(&mut self, bytes: impl Into<bytes::Bytes>) -> io::Result<()> {
        let bytes = bytes.into();
        if self.offset != self.bytes.len() {
            return Err(io::Error::other(
                "previous transport chunk is still pending",
            ));
        }
        if bytes.len() > crate::IO_CHUNK_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "transport chunk is too large",
            ));
        }
        self.bytes = bytes;
        self.offset = 0;
        Ok(())
    }
    pub async fn flush(&mut self) -> io::Result<bool> {
        let Some(fd) = self.fd.as_ref() else {
            return Ok(false);
        };
        while self.offset < self.bytes.len() {
            let mut ready = fd.writable().await?;
            match ready.try_io(|fd| {
                rustix::io::write(fd, &self.bytes[self.offset..]).map_err(io::Error::from)
            }) {
                Ok(Ok(0)) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(Ok(count)) => self.offset += count,
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => {}
                Ok(Err(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
                    self.fd.take();
                    self.bytes = bytes::Bytes::new();
                    self.offset = 0;
                    return Ok(false);
                }
                Ok(Err(error)) => return Err(error),
                Err(_) => {}
            }
        }
        self.bytes = bytes::Bytes::new();
        self.offset = 0;
        Ok(true)
    }
}
