//! Gated process-group launch and single-owner child observation.
use crate::{plan::Plan, sys};
use rustix::{
    fs::{Mode, OFlags, fcntl_getfl, fcntl_setfl, open},
    io::{Errno, fcntl_dupfd_cloexec, read, write},
    process::{
        Pid, Signal, WaitId, WaitIdOptions, WaitOptions, WaitStatus, kill_process_group, waitid,
        waitpid,
    },
};
use std::{
    collections::BTreeMap,
    ffi::CString,
    io::{self, IsTerminal},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    signal::unix::{SignalKind, signal},
};

/// Launch-time environment and cwd capabilities; neither mutates process-global state.
pub struct Snapshot {
    pub cwd: OwnedFd,
    pub environment: BTreeMap<CString, CString>,
}
impl Snapshot {
    /// Replace the session directory capability after validating the destination.
    ///
    /// # Errors
    /// Returns the OS error without changing the current capability.
    pub fn set_cwd(&mut self, path: &Path) -> io::Result<()> {
        let directory = crate::source::Directory::relative(&self.cwd, path)?;
        let destination = cstring(directory.path()?.as_os_str().as_bytes())?;
        let previous = crate::source::directory_path(&self.cwd)
            .ok()
            .map(|path| cstring(path.as_os_str().as_bytes()))
            .transpose()?;
        self.cwd = directory.into_fd();
        self.environment.insert(c"PWD".into(), destination);
        if let Some(previous) = previous {
            self.environment.insert(c"OLDPWD".into(), previous);
        } else {
            self.environment.remove(c"OLDPWD");
        }
        Ok(())
    }

    /// Capture the native process context once when creating a shell session.
    ///
    /// # Errors
    /// Reports an inaccessible directory or invalid environment data.
    pub fn current() -> io::Result<Self> {
        let cwd = open(
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let mut environment = BTreeMap::new();
        for (name, value) in std::env::vars_os() {
            let name = name.as_bytes();
            if !name.is_empty() && !name.contains(&b'=') {
                environment
                    .entry(cstring(name)?)
                    .or_insert(cstring(value.as_bytes())?);
            }
        }
        match crate::source::directory_path(&cwd) {
            Ok(path) => {
                environment.insert(c"PWD".into(), cstring(path.as_os_str().as_bytes())?);
            }
            Err(_) => {
                environment.remove(c"PWD");
            }
        }
        Ok(Self { cwd, environment })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    Exited(i32),
    Signaled(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Stopped,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Foreground,
    Capture,
    Stream,
    Through,
    Background,
    /// Persistent output helper: stdin carries frames, stdout acknowledges writes.
    Output {
        stderr: bool,
    },
}

/// A launch barrier keeps the first group member alive until every stage has joined.
#[must_use = "a job must be explicitly finished or cancelled"]
pub struct Job {
    children: Vec<(Pid, Option<Termination>, bool)>,
    group: Option<Pid>,
    events: tokio::signal::unix::Signal,
    gate: Option<AsyncFd<OwnedFd>>,
    gate_written: usize,
    status: Option<AsyncFd<OwnedFd>>,
    status_bytes: Vec<u8>,
    ready: Vec<bool>,
    failed: Vec<bool>,
    cleanup_signals: Vec<(Pid, Signal)>,
    pub stdin: Option<AsyncFd<OwnedFd>>,
    pub stdout: Option<AsyncFd<OwnedFd>>,
    pub stderr: Option<AsyncFd<OwnedFd>>,
}
impl Job {
    /// Start gated helper processes with evaluated byte arguments and ordered redirects.
    /// The caller transfers terminal ownership before calling `commit`.
    ///
    /// # Errors
    /// On setup failure, all previously started helpers are killed and reaped.
    pub async fn prepare(
        launcher: &Path,
        plan: &Plan,
        snapshot: &Snapshot,
        mode: LaunchMode,
    ) -> io::Result<Self> {
        let (gate_read, gate_write) = crate::sys::pipe()?;
        let (status_read, status_write) = crate::sys::pipe()?;
        let mut job = Self {
            children: Vec::new(),
            group: None,
            events: signal(SignalKind::child())?,
            gate: Some(readiness(gate_write)?),
            gate_written: 0,
            status: Some(readiness(status_read)?),
            status_bytes: Vec::new(),
            ready: vec![false; plan.stages.len()],
            failed: vec![false; plan.stages.len()],
            cleanup_signals: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
        };
        if let Err(error) =
            job.spawn_stages(launcher, plan, snapshot, mode, &gate_read, &status_write)
        {
            return match job.cancel().await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(io::Error::new(
                    error.kind(),
                    format!("{error}; cleanup: {cleanup}"),
                )),
            };
        }
        Ok(job)
    }
    fn spawn_stages(
        &mut self,
        launcher: &Path,
        plan: &Plan,
        snapshot: &Snapshot,
        mode: LaunchMode,
        gate: &OwnedFd,
        status: &OwnedFd,
    ) -> io::Result<()> {
        if plan.stages.is_empty() || !launcher.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "launch needs a nonempty plan and absolute helper path",
            ));
        }
        let launcher = cstring(launcher.as_os_str().as_bytes())?;
        let pipes = (1..plan.stages.len())
            .map(|_| crate::sys::pipe())
            .collect::<Result<Vec<_>, _>>()?;
        let output = matches!(
            mode,
            LaunchMode::Capture
                | LaunchMode::Stream
                | LaunchMode::Through
                | LaunchMode::Output { .. }
        )
        .then(crate::sys::pipe)
        .transpose()?;
        let errors = (mode == LaunchMode::Capture
            || (matches!(mode, LaunchMode::Stream | LaunchMode::Through)
                && std::io::stderr().is_terminal()))
        .then(crate::sys::pipe)
        .transpose()?;
        let input_pipe = matches!(mode, LaunchMode::Through | LaunchMode::Output { .. })
            .then(crate::sys::pipe)
            .transpose()?;
        for (index, stage) in plan.stages.iter().enumerate() {
            if stage.argv.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "empty stage argv",
                ));
            }
            let cwd = duplicate(snapshot.cwd.as_fd())?;
            let input = if index == 0
                && let Some((read, _)) = &input_pipe
            {
                duplicate(read.as_fd())?
            } else if index == 0 && mode != LaunchMode::Foreground {
                open("/dev/null", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?
            } else if index == 0 {
                duplicate(std::io::stdin().as_fd())?
            } else {
                duplicate(pipes[index - 1].0.as_fd())?
            };
            let stdout = if index + 1 < plan.stages.len() {
                duplicate(pipes[index].1.as_fd())?
            } else if let Some((_, write)) = &output {
                duplicate(write.as_fd())?
            } else {
                duplicate(std::io::stdout().as_fd())?
            };
            let stderr = if let Some((_, write)) = &errors {
                duplicate(write.as_fd())?
            } else if mode == (LaunchMode::Output { stderr: false }) {
                duplicate(std::io::stdout().as_fd())?
            } else {
                duplicate(std::io::stderr().as_fd())?
            };
            let streams = [input, stdout, stderr];
            let environment = environment(snapshot, stage)?;
            let mut argv = vec![
                launcher.clone(),
                cstring(b"--internal-exec")?,
                cstring(gate.as_raw_fd().to_string().as_bytes())?,
                cstring(status.as_raw_fd().to_string().as_bytes())?,
                cstring(cwd.as_raw_fd().to_string().as_bytes())?,
                cstring(index.to_string().as_bytes())?,
            ];
            argv.extend(crate::helper::setup::arguments(stage)?);
            let duplicates = [
                (streams[0].as_fd(), 0),
                (streams[1].as_fd(), 1),
                (streams[2].as_fd(), 2),
                (gate.as_fd(), gate.as_raw_fd()),
                (status.as_fd(), status.as_raw_fd()),
                (cwd.as_fd(), cwd.as_raw_fd()),
            ];
            let pid = sys::spawn(&launcher, &argv, &environment, &duplicates, self.group)?;
            self.group.get_or_insert(pid);
            self.children.push((pid, None, false));
        }
        self.stdin = input_pipe.map(|(_, write)| readiness(write)).transpose()?;
        self.stdout = output.map(|(read, _)| readiness(read)).transpose()?;
        self.stderr = errors.map(|(read, _)| readiness(read)).transpose()?;
        Ok(())
    }
    #[must_use]
    pub const fn group(&self) -> Option<Pid> {
        self.group
    }

    /// Release every stage and distinguish exec failures from target exit code 127.
    ///
    /// # Errors
    /// Reports the failed stage and OS error; the caller must cancel the retained job.
    pub async fn commit(&mut self) -> io::Result<()> {
        while self.ready.iter().any(|ready| !ready) {
            if !self.receive_status().await? {
                return Err(io::Error::other("launch helper exited before readiness"));
            }
        }
        if let Some(gate) = &self.gate {
            let release = vec![1; self.children.len()];
            while self.gate_written < release.len() {
                let remaining = &release[self.gate_written..];
                match gate
                    .async_io(Interest::WRITABLE, |fd| Ok(write(fd, remaining)?))
                    .await
                {
                    Ok(0) => return Err(io::Error::other("launch gate closed")),
                    Ok(count) => self.gate_written += count,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
        }
        self.gate.take();
        while self.receive_status().await? {}
        Ok(())
    }
    async fn receive_status(&mut self) -> io::Result<bool> {
        if self.status.is_none() {
            return Ok(false);
        }
        let chunk = self.launch_status().await?;
        if chunk.is_empty() {
            self.status.take();
            if !self.status_bytes.is_empty() {
                return Err(io::Error::other("incomplete launch status record"));
            }
            return Ok(false);
        }
        self.status_bytes.extend(chunk);
        let (records, remainder) = self.status_bytes.as_chunks::<8>();
        for record in records {
            let stage = i32::from_le_bytes(record[..4].try_into().map_err(io::Error::other)?);
            let code = i32::from_le_bytes(record[4..].try_into().map_err(io::Error::other)?);
            let ready = usize::try_from(stage)
                .ok()
                .and_then(|i| self.ready.get_mut(i))
                .ok_or_else(|| io::Error::other("invalid launch stage index"))?;
            if code != 0 {
                self.failed[usize::try_from(stage).map_err(io::Error::other)?] = true;
                let error = io::Error::from_raw_os_error(code);
                return Err(io::Error::new(
                    error.kind(),
                    format!("stage {stage}: {error}"),
                ));
            }
            if *ready {
                return Err(io::Error::other("duplicate launch readiness record"));
            }
            *ready = true;
        }
        let consumed = self.status_bytes.len() - remainder.len();
        self.status_bytes.drain(..consumed);
        Ok(true)
    }
    async fn launch_status(&mut self) -> io::Result<Vec<u8>> {
        loop {
            // Drain records before interpreting SIGCHLD: a failed helper writes its
            // precise OS error before exiting, and readiness notifications may race.
            match read_available(self.status.as_ref().expect("launch status channel")) {
                Ok(chunk) => return Ok(chunk),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            if self.poll()? == State::Stopped {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "launch setup stopped",
                ));
            }
            if self
                .children
                .iter()
                .zip(&self.ready)
                .any(|((_, exit, _), ready)| exit.is_some() && !ready)
            {
                // A helper can write and exit between the first read and poll.
                // Once its exit is observed, any preceding error record is readable.
                match read_available(self.status.as_ref().expect("launch status channel")) {
                    Ok(chunk) => return Ok(chunk),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        return Err(io::Error::other("launch helper exited before readiness"));
                    }
                    Err(error) => return Err(error),
                }
            }
            tokio::select! {
                chunk = read_chunk(self.status.as_ref().expect("launch status channel")) => return chunk,
                event = self.events.recv() => {
                    event.ok_or_else(|| io::Error::other("child signal service closed"))?;
                }
            }
        }
    }
    /// Drain all available child states; notifications are wakeups rather than event counts.
    ///
    /// # Errors
    /// Reports unexpected child observation errors.
    pub fn poll(&mut self) -> io::Result<State> {
        for (pid, termination, stopped) in &mut self.children {
            if termination.is_some() {
                continue;
            }
            if Some(*pid) == self.group {
                observe_leader(*pid, termination, stopped)?;
                continue;
            }
            loop {
                match waitpid(
                    Some(*pid),
                    WaitOptions::NOHANG | WaitOptions::UNTRACED | WaitOptions::CONTINUED,
                ) {
                    Ok(Some((_, status))) => observe(status, termination, stopped),
                    Ok(None) => break,
                    Err(Errno::INTR) => {}
                    Err(error) => return Err(error.into()),
                }
                if termination.is_some() {
                    break;
                }
            }
        }
        if self.children.iter().all(|(_, done, _)| done.is_some()) {
            Ok(State::Finished)
        } else if self
            .children
            .iter()
            .all(|(_, done, stopped)| done.is_some() || *stopped)
        {
            Ok(State::Stopped)
        } else {
            Ok(State::Running)
        }
    }
    /// Wait for completion or a job-control stop without blocking the runtime thread.
    ///
    /// # Errors
    /// Reports child observation or closed signal service errors.
    pub async fn wait(&mut self) -> io::Result<State> {
        loop {
            let state = self.poll()?;
            if state != State::Running {
                return Ok(state);
            }
            // One stopped stage is enough to freeze the whole owned pipeline,
            // including peers that ignore terminal-generated SIGTSTP.
            if self
                .children
                .iter()
                .any(|(_, done, stopped)| done.is_none() && *stopped)
            {
                self.signal(Signal::STOP)?;
            }
            self.events
                .recv()
                .await
                .ok_or_else(|| io::Error::other("child signal service closed"))?;
        }
    }
    /// Send a group signal to the retained job.
    ///
    /// # Errors
    /// Reports an OS signal error other than an already completed group.
    pub fn signal(&self, signal: Signal) -> io::Result<()> {
        if let Some(group) = self.group {
            match kill_process_group(group, signal) {
                Ok(()) | Err(Errno::SRCH) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
    /// Continue an owned stopped group and clear its already observed stop state.
    ///
    /// # Errors
    /// Reports a failed group signal.
    pub fn resume(&mut self) -> io::Result<()> {
        self.signal(Signal::CONT)?;
        for (_, _, stopped) in &mut self.children {
            *stopped = false;
        }
        Ok(())
    }
    /// Terminate and reap every owned child, allowing a bounded graceful shutdown.
    /// Stopped children receive SIGCONT after SIGTERM so their handlers can run.
    ///
    /// # Errors
    /// Reports signaling or reaping failure.
    pub async fn cancel(&mut self) -> io::Result<()> {
        let running = self.poll()? != State::Finished;
        if running && !self.failed.iter().all(|failed| *failed) {
            // An exiting Darwin process can briefly reject signals before waitid
            // exposes its exit. Observe completion through the grace period.
            let _ = self.cleanup_signal(Signal::TERM);
            if self
                .children
                .iter()
                .any(|(_, done, stopped)| done.is_none() && *stopped)
            {
                let _ = self.signal(Signal::CONT);
            }
        }
        self.gate.take();
        self.status.take();
        let deadline = tokio::time::sleep(std::time::Duration::from_secs(1));
        tokio::pin!(deadline);
        let mut escalated = false;
        while self.poll()? != State::Finished {
            tokio::select! {
                event = self.events.recv() => {
                    event.ok_or_else(|| io::Error::other("child signal service closed"))?;
                }
                () = &mut deadline, if !escalated => {
                    self.signal_live(Signal::KILL)?;
                    escalated = true;
                }
            }
        }
        self.reap()
    }
    fn signal_live(&mut self, signal: Signal) -> io::Result<()> {
        if let Err(error) = self.cleanup_signal(signal) {
            // Darwin can report EPERM when only an exiting, unreaped leader remains.
            // Recheck ownership state rather than hiding a failure for a live group.
            if self.poll()? != State::Finished {
                return Err(error);
            }
        }
        Ok(())
    }
    fn cleanup_signal(&mut self, signal: Signal) -> io::Result<()> {
        self.signal(signal)?;
        self.cleanup_signals.extend(
            self.children
                .iter()
                .filter_map(|(pid, done, _)| done.is_none().then_some((*pid, signal))),
        );
        Ok(())
    }
    /// Whether a stage's recorded termination matches a signal sent for owned cleanup.
    #[must_use]
    pub fn cleanup_termination(&self, index: usize) -> bool {
        let (pid, termination, _) = self.children[index];
        matches!(termination, Some(Termination::Signaled(raw)) if self.cleanup_signals.iter().any(|(target, signal)| *target == pid && signal.as_raw() == raw))
    }
    /// Reap the completed group leader after transport and cleanup are finished.
    /// Until then, its unreaped PID prevents a signal from targeting a reused group id.
    ///
    /// # Errors
    /// Rejects premature reaping and reports wait errors.
    pub fn reap(&mut self) -> io::Result<()> {
        if self.children.iter().any(|(_, status, _)| status.is_none()) {
            return Err(io::Error::other("job still has live stages"));
        }
        if let Some(group) = self.group {
            loop {
                match waitpid(Some(group), WaitOptions::NOHANG) {
                    Ok(Some(_)) => {
                        self.group = None;
                        break;
                    }
                    Err(Errno::INTR) => {}
                    Ok(None) => return Err(io::Error::other("group leader is not ready to reap")),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }
    /// Whether an observed stage terminated from the terminal's interrupt signal.
    #[must_use]
    pub fn interrupted(&self) -> bool {
        self.children.iter().any(|(_, status, _)| {
            matches!(status, Some(Termination::Signaled(signal)) if *signal == Signal::INT.as_raw())
        })
    }
    #[must_use]
    pub fn terminations(&self) -> Vec<Option<Termination>> {
        self.children.iter().map(|(_, status, _)| *status).collect()
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if self.children.iter().any(|(_, status, _)| status.is_none()) {
            let _ = self.signal(Signal::KILL);
            let _ = self.poll();
        }
        let _ = self.reap();
    }
}
fn observe(status: WaitStatus, termination: &mut Option<Termination>, stopped: &mut bool) {
    if let Some(code) = status.exit_status() {
        *termination = Some(Termination::Exited(code));
    } else if let Some(signal) = status.terminating_signal() {
        *termination = Some(Termination::Signaled(signal));
    } else if status.stopped() {
        *stopped = true;
    } else if status.continued() {
        *stopped = false;
    }
}
fn duplicate(fd: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    Ok(fcntl_dupfd_cloexec(fd, 3)?)
}
fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}
fn readiness(fd: OwnedFd) -> io::Result<AsyncFd<OwnedFd>> {
    fcntl_setfl(&fd, fcntl_getfl(&fd)? | OFlags::NONBLOCK)?;
    AsyncFd::new(fd)
}
/// Read one ready pipe chunk. Empty output means EOF; EINTR and readiness races retry.
///
/// # Errors
/// Reports transport errors from the descriptor.
pub async fn read_chunk(fd: &AsyncFd<OwnedFd>) -> io::Result<Vec<u8>> {
    loop {
        match fd
            .async_io(Interest::READABLE, |fd| read_available(fd))
            .await
        {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

fn read_available(fd: impl AsFd) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; crate::IO_CHUNK_BYTES];
    let count = read(fd, &mut bytes)?;
    bytes.truncate(count);
    Ok(bytes)
}

fn observe_leader(
    pid: Pid,
    termination: &mut Option<Termination>,
    stopped: &mut bool,
) -> io::Result<()> {
    loop {
        let status = match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED
                | WaitIdOptions::STOPPED
                | WaitIdOptions::CONTINUED
                | WaitIdOptions::NOHANG
                | WaitIdOptions::NOWAIT,
        ) {
            Ok(Some(status)) => status,
            Ok(None) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        };
        if let Some(code) = status.exit_status() {
            *termination = Some(Termination::Exited(code));
            return Ok(());
        }
        if let Some(signal) = status.terminating_signal() {
            *termination = Some(Termination::Signaled(signal));
            return Ok(());
        }
        // Consume only stop/continue events; an exit remains unreaped even if it races this call.
        match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::STOPPED | WaitIdOptions::CONTINUED | WaitIdOptions::NOHANG,
        ) {
            Ok(Some(status)) => {
                if status.stopped() {
                    *stopped = true;
                } else if status.continued() {
                    *stopped = false;
                }
            }
            Ok(None) | Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Whether a recorded termination was the platform's broken-pipe signal.
#[must_use]
pub const fn is_sigpipe(termination: Termination) -> bool {
    matches!(termination, Termination::Signaled(signal) if signal == Signal::PIPE.as_raw())
}

fn environment(snapshot: &Snapshot, stage: &crate::plan::Stage) -> io::Result<Vec<CString>> {
    snapshot
        .environment
        .iter()
        .filter(|(key, _)| !stage.environment.contains_key(*key))
        .chain(
            stage
                .environment
                .iter()
                .filter_map(|(key, value)| value.as_ref().map(|value| (key, value))),
        )
        .map(|(key, value)| {
            let mut bytes = Vec::with_capacity(key.as_bytes().len() + value.as_bytes().len() + 1);
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(b'=');
            bytes.extend_from_slice(value.as_bytes());
            CString::new(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        })
        .collect()
}
