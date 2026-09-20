//! Evaluated launch descriptions contain owned bytes, never syntax or language values.
use std::{collections::BTreeMap, ffi::CString, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Plan {
    pub stages: Vec<Stage>,
}

#[derive(Clone, Debug)]
pub struct Stage {
    /// The executable is `argv[0]`; every argument has already passed NUL validation.
    pub argv: Vec<CString>,
    pub redirects: Vec<Redirect>,
    pub cwd: Option<PathBuf>,
    pub environment: BTreeMap<CString, CString>,
    pub accepted_codes: Vec<u8>,
}
impl Default for Stage {
    fn default() -> Self {
        Self {
            argv: Vec::new(),
            redirects: Vec::new(),
            cwd: None,
            environment: BTreeMap::new(),
            accepted_codes: vec![0],
        }
    }
}

#[derive(Clone, Debug)]
pub enum Redirect {
    Read(PathBuf),
    Write {
        stream: Output,
        path: PathBuf,
        append: bool,
    },
    ErrorToOutput,
}

/// The two output streams supported by Rill redirection syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Stdout,
    Stderr,
}
