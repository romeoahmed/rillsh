//! Owned service messages cross VM quanta without carrying arena references.
//!
//! Requests transfer work to the coordinator; responses transfer owned results back.
//! A selected `index` refers to the caller's requested source order, not a slot-map
//! position. The coordinator retains pending work when a wait future is dropped and
//! closes it explicitly when its evaluation ends.
use rill_system::{job::Termination, plan::Plan};
use std::{ffi::CString, path::PathBuf};

#[derive(Clone, Copy, Debug)]
pub enum RunMode {
    Checked,
    Report,
    Capture,
}

#[derive(Clone, Copy, Debug)]
pub enum Control {
    Wait,
    Foreground,
    Background,
    Cancel,
}

#[derive(Debug)]
pub enum Request {
    /// Start an owned callback operation and return its readiness identity.
    Defer(Box<Self>),
    OpenModule {
        directory: rill_system::source::Directory,
        path: PathBuf,
    },
    ReadModule(rill_system::source::SourceFile),
    Glob(CString),
    Start(Plan),
    Jobs,
    Control {
        id: i64,
        operation: Control,
    },
    Stdin,
    Files(PathBuf),
    Stream(Plan),
    Through(Plan),
    Send {
        key: rill_system::resources::SourceId,
        bytes: Option<bytes::Bytes>,
    },
    ReadSources {
        keys: Vec<rill_system::resources::SourceId>,
        wait: bool,
    },
    Close(Vec<rill_system::resources::SourceId>),
    Run {
        plan: Plan,
        mode: RunMode,
        max_bytes: usize,
    },
    Write(bytes::Bytes),
    Display(String),
    ReadText {
        path: PathBuf,
        max_bytes: usize,
    },
    WithCwd {
        path: PathBuf,
        plan: Plan,
    },
    Cd(PathBuf),
    Pwd,
    GetEnv(CString),
    SetEnv {
        name: CString,
        value: CString,
    },
    UnsetEnv(CString),
    Args,
    Exit {
        code: u8,
        force: bool,
    },
}

pub struct JobSnapshot {
    pub id: i64,
    pub kind: &'static str,
    pub state: &'static str,
}

pub enum Response {
    JobStopped,
    Deferred(rill_system::resources::SourceId),
    Operation {
        index: usize,
        result: Result<Box<Self>, rill_system::resources::SourceError>,
    },
    ReadFailure {
        index: usize,
        error: Box<rill_system::resources::SourceError>,
    },
    ModuleFile(rill_system::source::SourceFile),
    ModuleText {
        file: rill_system::source::SourceFile,
        text: String,
    },
    Paths(Vec<PathBuf>),
    Job(i64),
    Jobs(Vec<JobSnapshot>),
    Source(rill_system::resources::SourceId),
    Through {
        output: rill_system::resources::SourceId,
        input: rill_system::resources::SourceId,
    },
    /// A nonblocking source poll found no ready item; no input has reached EOF.
    Pending,
    Item {
        index: usize,
        /// `None` is EOF for this selected source, not for its ready peers.
        value: Option<rill_system::resources::Item>,
    },
    Unit,
    Text(String),
    Path(PathBuf),
    OptionalBytes(Option<Vec<u8>>),
    Arguments(Vec<Vec<u8>>),
    Plan(Plan),
    Run {
        id: i64,
        mode: RunMode,
        plan: Plan,
        terminations: Vec<Termination>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        cancelled: bool,
    },
}
