//! Callback waits own their jobs and workers independently of the currently selected branch.
use super::{Session, execution::Running, files};
use rill_runtime::{
    Error,
    host::{Control, Request, Response, RunMode},
};
use rill_system::resources::{SourceError, SourceId};
use std::{collections::HashMap, io};

pub(super) enum Ready {
    Source(Result<Option<(usize, Option<rill_system::resources::Item>)>, SourceError>),
    Operation(Option<usize>, SourceId, io::Result<()>),
}

pub(super) async fn readiness(
    sources: &mut rill_system::resources::Sources,
    operations: &mut Operations,
    keys: &[SourceId],
    wait: bool,
    terminal: Option<&rill_system::terminal::Terminal>,
    jobs: &mut super::jobs::Jobs,
) -> Ready {
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    if operations.entries.is_empty() {
        return Ready::Source(sources.next_ready(keys, wait).await);
    }
    let sources = sources.next_ready(keys, true);
    let operations = operations.ready(keys, terminal, jobs);
    tokio::pin!(sources, operations);
    poll_fn(|cx| {
        if let Poll::Ready((index, key, result)) = operations.as_mut().poll(cx) {
            return Poll::Ready(Ready::Operation(index, key, result));
        }
        if let Poll::Ready(result) = sources.as_mut().poll(cx) {
            return Poll::Ready(Ready::Source(result));
        }
        if wait {
            Poll::Pending
        } else {
            Poll::Ready(Ready::Source(Ok(None)))
        }
    })
    .await
}

#[derive(Default)]
pub(super) struct Operations {
    entries: HashMap<SourceId, Operation>,
    foreground: std::collections::VecDeque<SourceId>,
}
enum Operation {
    Control {
        id: i64,
        foreground: bool,
        started: bool,
    },
    Settled(Result<Response, SourceError>),
    Write {
        writer: Box<rill_system::writer::Writer>,
        stderr: bool,
    },
    Run(Box<Running>),
    Glob(Box<Running>),
    Launch(Box<super::launch::Launching>),
    File(tokio::task::JoinHandle<io::Result<Product>>),
    Ready(io::Result<Product>),
}
enum Product {
    Reply(Response),
    Directory(rill_system::resources::DirectorySource),
}
impl Operation {
    async fn ready(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
    ) -> io::Result<()> {
        match self {
            Self::Run(run) | Self::Glob(run) => run.advance(terminal).await,
            Self::Settled(_) | Self::Ready(_) => Ok(()),
            Self::Control { .. } => unreachable!("job table owns control polling"),
            Self::Launch(launch) => launch.job.commit().await,
            Self::Write { writer, .. } => writer.flush().await,
            Self::File(worker) => {
                *self = Self::Ready(
                    worker
                        .await
                        .map_err(io::Error::other)
                        .and_then(std::convert::identity),
                );
                Ok(())
            }
        }
    }
    async fn cancel(self, jobs: &mut super::jobs::Jobs) -> io::Result<()> {
        match self {
            Self::Control {
                id,
                foreground: true,
                started: true,
            } => {
                let entry = jobs.entries.get_mut(&id).expect("retained controlled job");
                entry.cancelled = true;
                entry.acknowledged = true;
                entry.job.cancel().await
            }
            Self::Control { .. } => Ok(()),
            Self::Run(mut run) | Self::Glob(mut run) => run.job.cancel().await,
            Self::Launch(mut launch) => launch.cancel().await,
            Self::Write { mut writer, .. } => writer.cancel().await,
            Self::File(worker) => {
                drop(worker.await.map_err(io::Error::other)??);
                Ok(())
            }
            Self::Ready(result) => result.map(drop),
            Self::Settled(result) => result.map(drop).map_err(io::Error::other),
        }
    }
}
impl Operations {
    pub fn foreground_job(&self) -> Option<i64> {
        self.entries.values().find_map(|operation| {
            if let Operation::Control {
                id,
                foreground: true,
                started: true,
            } = operation
            {
                Some(*id)
            } else {
                None
            }
        })
    }

    pub fn reads_stdin(&self) -> bool {
        self.entries
            .values()
            .any(|operation| matches!(operation, Operation::Run(run) if run.reads_stdin()))
    }

    pub async fn ready(
        &mut self,
        keys: &[SourceId],
        terminal: Option<&rill_system::terminal::Terminal>,
        jobs: &mut super::jobs::Jobs,
    ) -> (Option<usize>, SourceId, io::Result<()>) {
        use std::{
            future::{Future, poll_fn},
            task::Poll,
        };
        let positions: HashMap<_, _> = keys
            .iter()
            .enumerate()
            .map(|(index, key)| (*key, index))
            .collect();
        let active = self.foreground.front().copied();
        let mut pending = Vec::new();
        let mut controls = Vec::new();
        for (key, operation) in &mut self.entries {
            let foreground = terminal.is_some()
                && matches!(operation, Operation::Run(run) if !run.captures())
                || terminal.is_some()
                    && matches!(
                        operation,
                        Operation::Control {
                            foreground: true,
                            ..
                        }
                    );
            if foreground && active != Some(*key) {
                continue;
            }
            let index = positions.get(key).copied();
            if index.is_none() && active != Some(*key) {
                continue;
            }
            if let Operation::Control {
                id,
                foreground,
                started,
            } = operation
            {
                controls.push((index, *key, *id, *foreground, started));
            } else {
                pending.push((index, *key, Box::pin(operation.ready(terminal))));
            }
        }
        // Preserve the caller's polling order across both native jobs and owned futures.
        let mut order: Vec<_> = pending
            .iter()
            .enumerate()
            .map(|(slot, (index, ..))| (*index, false, slot))
            .chain(
                controls
                    .iter()
                    .enumerate()
                    .map(|(slot, (index, ..))| (*index, true, slot)),
            )
            .collect();
        order.sort_unstable_by_key(|(index, ..)| *index);
        poll_fn(|cx| {
            for &(_, control, slot) in &order {
                if !control {
                    let (index, key, operation) = &mut pending[slot];
                    if let Poll::Ready(result) = operation.as_mut().poll(cx) {
                        return Poll::Ready((*index, *key, result));
                    }
                    continue;
                }
                let (index, key, id, foreground, started) = &mut controls[slot];
                let entry = jobs.entries.get_mut(id).expect("retained controlled job");
                let result = (|| {
                    let mut state = entry.job.poll()?;
                    if *foreground && !**started && state != rill_system::job::State::Finished {
                        entry.foreground_native(terminal)?;
                        **started = true;
                        state = entry.job.poll()?;
                    }
                    match state {
                        rill_system::job::State::Running => Ok(false),
                        rill_system::job::State::Stopped if *foreground => Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "foreground job stopped",
                        )),
                        _ => Ok(true),
                    }
                })();
                match result {
                    Ok(false) => {}
                    result => return Poll::Ready((*index, *key, result.map(|_| ()))),
                }
            }
            Poll::Pending
        })
        .await
    }
    pub async fn close(
        &mut self,
        keys: &[SourceId],
        terminal: Option<&rill_system::terminal::Terminal>,
        jobs: &mut super::jobs::Jobs,
    ) -> Result<(), SourceError> {
        let mut primary = None;
        let mut notes = Vec::new();
        let foreground = self
            .foreground
            .front()
            .is_some_and(|key| keys.contains(key));
        for key in keys {
            self.foreground.retain(|entry| entry != key);
            if let Some(operation) = self.entries.remove(key)
                && let Err(error) = operation.cancel(jobs).await
            {
                if primary.is_none() {
                    primary = Some(error);
                } else {
                    notes.push(error.to_string());
                }
            }
        }
        if foreground
            && let Some(terminal) = terminal
            && let Err(error) = terminal.restore()
        {
            if primary.is_none() {
                primary = Some(error);
            } else {
                notes.push(error.to_string());
            }
        }
        primary.map_or(Ok(()), |error| {
            Err(SourceError::Cleanup {
                primary: Box::new(error.into()),
                notes,
            })
        })
    }
    pub async fn close_all(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
        jobs: &mut super::jobs::Jobs,
    ) -> Result<(), SourceError> {
        self.close(
            &self.entries.keys().copied().collect::<Vec<_>>(),
            terminal,
            jobs,
        )
        .await
    }
    pub async fn suspend(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
        jobs: &mut super::jobs::Jobs,
    ) -> io::Result<()> {
        for (key, operation) in &mut self.entries {
            match operation {
                Operation::Control {
                    id,
                    foreground: true,
                    started: true,
                } => {
                    let entry = jobs.entries.get_mut(id).expect("attached foreground job");
                    entry.job.signal(rustix::process::Signal::STOP)?;
                    entry.job.wait().await?;
                    if let Some(terminal) = terminal {
                        entry.modes = Some(terminal.modes()?);
                    }
                }
                Operation::Run(run) | Operation::Glob(run) => {
                    if self.foreground.front() == Some(key)
                        && let Some(terminal) = terminal
                    {
                        run.suspend(terminal).await?;
                    } else {
                        run.job.signal(rustix::process::Signal::STOP)?;
                        run.job.wait().await?;
                    }
                }
                Operation::Launch(launch) => {
                    launch.suspend().await?;
                }
                Operation::File(_) => {
                    operation.ready(None).await?;
                }
                Operation::Control { .. } | Operation::Ready(_) | Operation::Settled(_) => {}
                Operation::Write { writer, .. } => writer.suspend().await?,
            }
        }
        Ok(())
    }
    pub fn resume(
        &mut self,
        terminal: Option<&rill_system::terminal::Terminal>,
        jobs: &mut super::jobs::Jobs,
    ) -> io::Result<()> {
        for (key, operation) in &mut self.entries {
            match operation {
                Operation::Control {
                    id,
                    foreground: true,
                    started: true,
                } => {
                    jobs.entries
                        .get_mut(id)
                        .expect("attached foreground job")
                        .foreground_native(terminal)?;
                }
                Operation::Run(run) | Operation::Glob(run) => {
                    if self.foreground.front() == Some(key)
                        && let Some(terminal) = terminal
                    {
                        run.resume(terminal)?;
                    } else {
                        run.job.resume()?;
                    }
                }
                Operation::Launch(launch) => launch.resume()?,
                Operation::Write { writer, .. } => writer.resume()?,
                _ => {}
            }
        }
        Ok(())
    }
}
impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "Only the current-thread coordinator can publish completed resources into the evaluation"
    )]
    pub(super) async fn complete_operation(
        &mut self,
        key: SourceId,
        result: io::Result<()>,
    ) -> Result<Response, SourceError> {
        let operation = self
            .operations
            .entries
            .remove(&key)
            .expect("ready operation remains owned");
        let foreground = self.operations.foreground.front() == Some(&key);
        self.operations.foreground.retain(|entry| *entry != key);
        let glob = matches!(&operation, Operation::Glob(_));
        let result = async {
            if let Err(error) = result {
                return Err(match operation.cancel(&mut self.jobs).await {
                    Ok(()) => error.into(),
                    Err(cleanup) => SourceError::Cleanup {
                        primary: Box::new(error.into()),
                        notes: vec![cleanup.to_string()],
                    },
                });
            }
            match operation {
                Operation::Settled(result) => result,
                Operation::Control { id, foreground, .. } => {
                    let entry = self
                        .jobs
                        .entries
                        .get_mut(&id)
                        .expect("controlled job remains in its session");
                    if entry.job.poll()? == rill_system::job::State::Stopped {
                        return Ok(Response::JobStopped);
                    }
                    if foreground && entry.job.interrupted() && !self.engine.cleaning() {
                        self.interrupted = true;
                        entry.acknowledged = true;
                    }
                    entry
                        .native_response(
                            id,
                            if foreground {
                                RunMode::Checked
                            } else {
                                RunMode::Report
                            },
                        )
                        .map_err(SourceError::from)
                }
                Operation::Run(mut run) | Operation::Glob(mut run) => {
                    if let Err(error) = run.job.reap() {
                        let cleanup = run.job.cancel().await;
                        return Err(match cleanup {
                            Ok(()) => error.into(),
                            Err(cleanup) => SourceError::Cleanup {
                                primary: Box::new(error.into()),
                                notes: vec![cleanup.to_string()],
                            },
                        });
                    }
                    if !run.captures() && run.job.interrupted() && !self.engine.cleaning() {
                        self.interrupted = true;
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "foreground evaluation cancelled",
                        )
                        .into());
                    }
                    let response = run.response()?;
                    if glob {
                        super::glob::response(response)
                    } else {
                        Ok(response)
                    }
                }
                Operation::Ready(result) => match result? {
                    Product::Reply(response) => Ok(response),
                    Product::Directory(directory) => {
                        Ok(Response::Source(self.sources.register_directory(directory)))
                    }
                },
                Operation::Launch(launch) => Ok(self.publish_launch(*launch)),
                Operation::Write { writer, stderr } => {
                    self.writers.reuse(stderr, *writer).await?;
                    Ok(Response::Unit)
                }
                Operation::File(_) => unreachable!("file readiness retains its result"),
            }
        }
        .await;
        if foreground
            && let Some(terminal) = &self.terminal
            && let Err(error) = terminal.restore()
        {
            return Err(match result {
                Ok(_) => error.into(),
                Err(primary) => SourceError::Cleanup {
                    primary: Box::new(primary),
                    notes: vec![error.to_string()],
                },
            });
        }
        result
    }
    pub(super) fn retain_operation(
        &mut self,
        key: SourceId,
        result: Result<Response, SourceError>,
    ) {
        self.operations
            .entries
            .insert(key, Operation::Settled(result));
    }

    #[expect(
        clippy::future_not_send,
        reason = "The coordinator lends an idle output helper to one callback"
    )]
    async fn defer_write(&mut self, bytes: bytes::Bytes, stderr: bool) -> Result<Response, Error> {
        let mut writer = match self.writers.take(stderr) {
            Some(writer) => writer,
            None => {
                rill_system::writer::Writer::prepare(&self.launcher, &self.snapshot, stderr).await?
            }
        };
        if let Err(error) = writer.begin(bytes) {
            let cleanup = writer.cancel().await;
            let mut error = Error::from_io(&error);
            if let Err(cleanup) = cleanup {
                error.notes.push(cleanup.to_string());
            }
            return Err(error);
        }
        let key = self.sources.external();
        self.operations.entries.insert(
            key,
            Operation::Write {
                writer: Box::new(writer),
                stderr,
            },
        );
        Ok(Response::Deferred(key))
    }
    #[expect(
        clippy::future_not_send,
        reason = "The coordinator starts callback operations without transferring traced state"
    )]
    pub(super) async fn defer(&mut self, request: Request) -> Result<Response, Error> {
        let request = match request {
            Request::Display(mut text) => {
                text.push('\n');
                return self.defer_write(text.into_bytes().into(), true).await;
            }
            Request::Write(bytes) => return self.defer_write(bytes, false).await,
            request => request,
        };
        let operation = match request {
            Request::Control {
                id,
                operation: operation @ (Control::Wait | Control::Foreground),
            } => {
                if self.retained_caller(id)
                    || self
                        .suspended
                        .values()
                        .any(|entry| entry.foreground_job() == Some(id))
                {
                    return Err(Error::new(
                        "JobError",
                        "job belongs to a retained evaluation",
                    ));
                }
                self.jobs.get(id)?;
                Operation::Control {
                    id,
                    foreground: matches!(operation, Control::Foreground),
                    started: false,
                }
            }
            Request::Glob(pattern) => Operation::Glob(Box::new(self.prepare_glob(pattern).await?)),
            Request::Run {
                plan,
                mode,
                max_bytes,
            } => Operation::Run(Box::new(self.prepare_run(plan, mode, max_bytes).await?)),
            Request::Files(path) => {
                let cwd = self.snapshot.cwd.try_clone()?;
                Operation::File(tokio::task::spawn_blocking(move || {
                    rill_system::resources::DirectorySource::open(&cwd, path)
                        .map(Product::Directory)
                }))
            }
            Request::Start(plan) => Operation::Launch(Box::new(
                self.prepare_launch(plan, rill_system::job::LaunchMode::Background)
                    .await?,
            )),
            Request::Stream(plan) => Operation::Launch(Box::new(
                self.prepare_launch(plan, rill_system::job::LaunchMode::Stream)
                    .await?,
            )),
            Request::Through(plan) => Operation::Launch(Box::new(
                self.prepare_launch(plan, rill_system::job::LaunchMode::Through)
                    .await?,
            )),
            Request::ReadText { path, max_bytes } => {
                let cwd = self.snapshot.cwd.try_clone()?;
                Operation::File(tokio::task::spawn_blocking(move || {
                    files::text(&cwd, &path, max_bytes).map(Product::Reply)
                }))
            }
            _ => return Err(Error::new("RuntimeError", "unsupported deferred operation")),
        };
        let key = self.sources.external();
        if self.terminal.is_some()
            && (matches!(&operation, Operation::Run(run) if !run.captures())
                || matches!(
                    &operation,
                    Operation::Control {
                        foreground: true,
                        ..
                    }
                ))
        {
            self.operations.foreground.push_back(key);
        }
        self.operations.entries.insert(key, operation);
        Ok(Response::Deferred(key))
    }
    #[expect(
        clippy::future_not_send,
        reason = "Operation cleanup completes before the coordinator resumes language cleanup"
    )]
    pub(super) async fn close_sources(&mut self, keys: Vec<SourceId>) -> Result<Response, Error> {
        let operations = self
            .operations
            .close(&keys, self.terminal.as_ref(), &mut self.jobs)
            .await;
        let sources = self.sources.close_many(keys).await;
        match (operations, sources) {
            (Ok(()), Ok(())) => Ok(Response::Unit),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error.into()),
            (Err(primary), Err(cleanup)) => {
                let mut error = Error::from(primary);
                error.notes.push(cleanup.to_string());
                Err(error)
            }
        }
    }
}
