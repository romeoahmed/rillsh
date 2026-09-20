//! Generational ownership for filesystem sources; blocking work remains owned until joined.
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};
use slotmap::{SlotMap, new_key_type};
use std::{
    ffi::OsString,
    io,
    os::{fd::OwnedFd, unix::ffi::OsStringExt},
    path::PathBuf,
};
use tokio::task::JoinHandle;
new_key_type! { pub struct SourceId; }

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{error}")]
    Selected { index: usize, error: Box<Self> },
    #[error("{primary}")]
    Cleanup {
        primary: Box<Self>,
        notes: Vec<String>,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("stage {stage} failed with status {code}")]
    Process { stage: usize, code: u8 },
    #[error("process source is stopped")]
    Stopped,
}
pub enum Item {
    Written(bool),
    Entry(Entry),
    Bytes(Vec<u8>),
}

pub struct Entry {
    pub name: PathBuf,
    pub path: PathBuf,
    pub kind: &'static str,
    pub size: i64,
}
/// An opened directory capability, transferable before it receives a source identity.
pub struct DirectorySource {
    dir: Dir,
    path: PathBuf,
}
impl DirectorySource {
    /// Open a directory relative to the supplied capability on a blocking worker.
    ///
    /// # Errors
    /// Reports directory access or stream initialization errors.
    pub fn open(cwd: &OwnedFd, path: PathBuf) -> io::Result<Self> {
        let fd = openat(
            cwd,
            &path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self {
            dir: Dir::new(fd)?,
            path,
        })
    }

    fn next(&mut self) -> io::Result<Option<Entry>> {
        while let Some(entry) = self.dir.read() {
            let entry = entry?;
            if matches!(entry.file_name().to_bytes(), b"." | b"..") {
                continue;
            }
            let metadata = statat(self.dir.fd()?, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW)?;
            let name = PathBuf::from(OsString::from_vec(entry.file_name().to_bytes().to_vec()));
            let kind = match FileType::from_raw_mode(metadata.st_mode) {
                FileType::RegularFile => "file",
                FileType::Directory => "directory",
                FileType::Symlink => "symlink",
                _ => "other",
            };
            return Ok(Some(Entry {
                path: self.path.join(&name),
                name,
                kind,
                size: metadata.st_size,
            }));
        }
        Ok(None)
    }
}
enum Resource {
    External,
    Output(crate::output::Output),
    Input(crate::input::Input),
    Process(Box<crate::process_source::ProcessSource>),
    Ready(Option<DirectorySource>),
    Reading(JoinHandle<(DirectorySource, io::Result<Option<Entry>>)>),
}
impl Resource {
    async fn next(&mut self) -> Result<Option<Item>, SourceError> {
        match self {
            Self::External => return std::future::pending().await,
            Self::Output(output) => {
                return output
                    .flush()
                    .await
                    .map(|open| Some(Item::Written(open)))
                    .map_err(SourceError::from);
            }
            Self::Input(source) => {
                return source
                    .next()
                    .await
                    .map(|bytes| (!bytes.is_empty()).then_some(Item::Bytes(bytes)))
                    .map_err(SourceError::from);
            }
            Self::Process(source) => {
                return source.next().await.map(|bytes| bytes.map(Item::Bytes));
            }
            Self::Ready(source) => {
                let mut source = source
                    .take()
                    .ok_or_else(|| io::Error::other("source is closed"))?;
                *self = Self::Reading(tokio::task::spawn_blocking(move || {
                    let result = source.next();
                    (source, result)
                }));
            }
            Self::Reading(_) => {}
        }
        let Self::Reading(worker) = self else {
            unreachable!("directory read")
        };
        let result = worker.await;
        *self = Self::Ready(None);
        let (source, result) = result.map_err(io::Error::other)?;
        *self = Self::Ready(Some(source));
        result
            .map(|entry| entry.map(Item::Entry))
            .map_err(SourceError::from)
    }
}
#[derive(Default)]
pub struct Sources {
    entries: SlotMap<SourceId, Resource>,
    stdin: Option<SourceId>,
}
impl Sources {
    /// Reserve a readiness identity whose operation is owned by the coordinator.
    pub fn external(&mut self) -> SourceId {
        self.entries.insert(Resource::External)
    }

    /// Whether an active source owns the session's stdin lease.
    #[must_use]
    pub fn stdin_leased(&self) -> bool {
        self.stdin.is_some_and(|key| self.entries.contains_key(key))
    }
    /// Borrow standard input without changing its status flags or closing descriptor 0.
    ///
    /// # Errors
    /// Rejects a second live lease or a failed descriptor duplication.
    pub fn stdin(&mut self) -> io::Result<SourceId> {
        use std::os::fd::AsFd;
        if self.stdin_leased() {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "stdin already has a live source",
            ));
        }
        let fd = rustix::io::fcntl_dupfd_cloexec(std::io::stdin().as_fd(), 3)?;
        let key = self
            .entries
            .insert(Resource::Input(crate::input::Input::new(fd)?));
        self.stdin = Some(key);
        Ok(key)
    }
    /// Transfer an already committed process job into a scoped source.
    /// If the job captures terminal stderr, `relay` must own its output helper.
    pub fn process(
        &mut self,
        job: crate::job::Job,
        plan: crate::plan::Plan,
        relay: Option<crate::writer::Writer>,
    ) -> SourceId {
        self.entries.insert(Resource::Process(Box::new(
            crate::process_source::ProcessSource::new(job, plan, relay),
        )))
    }
    /// Register the writable end of a through-job's stdin.
    pub fn output(&mut self, fd: tokio::io::unix::AsyncFd<OwnedFd>) -> SourceId {
        self.entries
            .insert(Resource::Output(crate::output::Output::new(fd)))
    }
    /// Queue one bounded chunk, or close the writer to deliver EOF.
    ///
    /// # Errors
    /// Rejects stale keys, non-writers, and overlapping chunks.
    pub fn send(&mut self, key: SourceId, bytes: Option<bytes::Bytes>) -> io::Result<()> {
        let Some(Resource::Output(output)) = self.entries.get_mut(key) else {
            return Err(io::Error::other("process input is closed"));
        };
        if let Some(bytes) = bytes {
            output.enqueue(bytes)
        } else {
            self.entries.remove(key);
            Ok(())
        }
    }
    /// Open a directory relative to the launch-independent session capability.
    ///
    /// # Errors
    /// Returns directory access or worker failure before publishing a key.
    pub async fn directory(&mut self, cwd: OwnedFd, path: PathBuf) -> io::Result<SourceId> {
        let source = tokio::task::spawn_blocking(move || DirectorySource::open(&cwd, path))
            .await
            .map_err(io::Error::other)??;
        Ok(self.register_directory(source))
    }
    /// Publish a successfully opened directory as a single-consumer source.
    pub fn register_directory(&mut self, source: DirectorySource) -> SourceId {
        self.entries.insert(Resource::Ready(Some(source)))
    }

    /// Pull one item, retaining an in-flight worker if the future is cancelled.
    ///
    /// # Errors
    /// Rejects stale keys and propagates source failures.
    pub async fn next(&mut self, key: SourceId) -> Result<Option<Item>, SourceError> {
        let resource = self
            .entries
            .get_mut(key)
            .ok_or_else(|| io::Error::other("source is closed"))?;
        let result = resource.next().await;
        if matches!(result, Ok(None)) {
            self.entries.remove(key);
        }
        result
    }
    // The common one-source case needs neither a hash table nor boxed readiness futures.
    async fn next_one(
        &mut self,
        key: SourceId,
        wait: bool,
    ) -> Result<Option<(usize, Option<Item>)>, SourceError> {
        use std::{
            future::{Future, poll_fn},
            task::Poll,
        };
        let resource = self.entries.get_mut(key).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid source selection")
        })?;
        let result = {
            let read = resource.next();
            tokio::pin!(read);
            poll_fn(|cx| match read.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    Poll::Ready(result.map(|item| Some((0, item))).map_err(|error| {
                        SourceError::Selected {
                            index: 0,
                            error: Box::new(error),
                        }
                    }))
                }
                Poll::Pending if wait => Poll::Pending,
                Poll::Pending => Poll::Ready(Ok(None)),
            })
            .await
        };
        if matches!(result, Ok(Some((_, None)))) {
            self.entries.remove(key);
        }
        result
    }
    /// Poll sources in the supplied order, returning at most one item or completion.
    /// Pending reads remain owned by their resources when another input wins.
    /// The returned index refers to `keys`; an inner `None` is that source's EOF.
    /// An outer `None` means no source is ready and occurs only when `wait` is false.
    ///
    /// # Errors
    /// Rejects an empty selection, stale keys or duplicates before polling any source.
    /// Source failures carry their selected index; selection errors have no index.
    pub async fn next_ready(
        &mut self,
        keys: &[SourceId],
        wait: bool,
    ) -> Result<Option<(usize, Option<Item>)>, SourceError> {
        use std::{
            collections::HashMap,
            future::{Future, poll_fn},
            task::Poll,
        };
        if let [key] = keys {
            return self.next_one(*key, wait).await;
        }
        let positions: HashMap<_, _> = keys.iter().enumerate().map(|(i, key)| (*key, i)).collect();
        if keys.is_empty()
            || positions.len() != keys.len()
            || keys.iter().any(|key| !self.entries.contains_key(*key))
        {
            return Err(
                io::Error::new(io::ErrorKind::InvalidInput, "invalid source selection").into(),
            );
        }
        let result = {
            // Keep readiness futures alive across Pending: dropping an AsyncFd wait
            // unregisters its waker even though the resource itself still exists.
            let mut reads: Vec<_> = self
                .entries
                .iter_mut()
                .filter_map(|(key, resource)| {
                    positions
                        .get(&key)
                        .map(|&index| (index, Box::pin(resource.next())))
                })
                .collect();
            reads.sort_unstable_by_key(|(index, _)| *index);
            poll_fn(|cx| {
                for (index, read) in &mut reads {
                    if let Poll::Ready(result) = read.as_mut().poll(cx) {
                        return Poll::Ready(result.map(|item| Some((*index, item))).map_err(
                            |error| SourceError::Selected {
                                index: *index,
                                error: Box::new(error),
                            },
                        ));
                    }
                }
                if wait {
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(None))
                }
            })
            .await
        };
        if let Ok(Some((index, None))) = &result {
            self.entries.remove(keys[*index]);
        }
        result
    }
    /// Freeze every owned process group, observing stop or exit before returning.
    /// Independent background jobs are not part of this resource set.
    ///
    /// # Errors
    /// Reports signaling or child-observation failures.
    pub async fn suspend(&mut self) -> io::Result<()> {
        for resource in self.entries.values_mut() {
            if let Resource::Process(source) = resource {
                source.job.signal(rustix::process::Signal::STOP)?;
            }
        }
        for resource in self.entries.values_mut() {
            match resource {
                Resource::Process(source) => {
                    source.suspend().await?;
                }
                Resource::Input(input) => input.pause().await?,
                _ => {}
            }
        }
        Ok(())
    }
    /// Continue scope-owned process groups without transferring terminal ownership.
    ///
    /// # Errors
    /// Reports a failed group signal.
    pub fn resume(&mut self) -> io::Result<()> {
        for resource in self.entries.values_mut() {
            if let Resource::Process(source) = resource {
                source.resume()?;
            }
        }
        Ok(())
    }
    /// Close a source. Repeated internal cleanup of an already removed key is harmless.
    ///
    /// # Errors
    /// Reports pending I/O, process cleanup or worker-join errors after removing the key.
    pub async fn close(&mut self, key: SourceId) -> Result<(), SourceError> {
        match self.entries.remove(key) {
            Some(Resource::Reading(worker)) => {
                drop(worker.await.map_err(io::Error::other)?.1?);
            }
            Some(Resource::Process(source)) => return source.close().await,
            Some(Resource::Input(source)) => source.close().await?,
            Some(Resource::Output(_) | Resource::Ready(_) | Resource::External) | None => {}
        }
        Ok(())
    }
    /// Close all sources, joining every outstanding worker even if one fails.
    ///
    /// # Errors
    /// Returns the first cleanup error after attempting every source.
    pub async fn close_all(&mut self) -> Result<(), SourceError> {
        let keys: Vec<_> = self.entries.keys().collect();
        self.close_many(keys).await
    }
    /// Close the specified resources, preserving the first error while completing cleanup.
    ///
    /// # Errors
    /// Returns the first failed worker join or pending I/O error.
    pub async fn close_many(&mut self, keys: Vec<SourceId>) -> Result<(), SourceError> {
        let mut errors = Vec::new();
        for key in keys {
            if let Err(error) = self.close(key).await {
                errors.push(error);
            }
        }
        SourceError::combine(errors)
    }
}

impl SourceError {
    pub(crate) fn combine(errors: impl IntoIterator<Item = Self>) -> Result<(), Self> {
        let mut errors = errors.into_iter();
        let Some(primary) = errors.next() else {
            return Ok(());
        };
        let notes: Vec<_> = errors
            .map(|error| match error {
                Self::Cleanup { primary, notes } => format!("{primary}; {}", notes.join("; ")),
                error => error.to_string(),
            })
            .collect();
        if notes.is_empty() {
            Err(primary)
        } else {
            Err(Self::Cleanup {
                primary: Box::new(primary),
                notes,
            })
        }
    }
}
