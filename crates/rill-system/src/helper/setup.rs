//! Ordered redirects are encoded once, then applied from borrowed helper arguments.
use crate::plan::{Output, Redirect, Stage};
use rustix::{
    fs::{Mode, OFlags, open},
    process::fchdir,
    stdio::{dup2_stderr, dup2_stdin, dup2_stdout, stdout},
};
use std::{
    ffi::{CString, OsStr},
    io,
    os::{fd::AsFd, unix::ffi::OsStrExt},
};

pub fn arguments(stage: &Stage) -> io::Result<Vec<CString>> {
    let mut arguments = Vec::new();
    if let Some(cwd) = &stage.cwd {
        arguments.extend([c"cwd".into(), path_argument(cwd.as_os_str())?]);
    }
    for redirect in &stage.redirects {
        let (tag, path) = match redirect {
            Redirect::Read(path) => (c"<", path),
            Redirect::Write {
                stream,
                path,
                append,
            } => (
                match (stream, append) {
                    (Output::Stdout, false) => c">",
                    (Output::Stdout, true) => c">>",
                    (Output::Stderr, false) => c"2>",
                    (Output::Stderr, true) => c"2>>",
                },
                path,
            ),
            Redirect::ErrorToOutput => {
                arguments.push(c"2>&1".into());
                continue;
            }
        };
        arguments.extend([tag.into(), path_argument(path.as_os_str())?]);
    }
    arguments.push(c"--".into());
    arguments.extend(stage.argv.iter().cloned());
    Ok(arguments)
}
fn path_argument(path: &OsStr) -> io::Result<CString> {
    CString::new(path.as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid launch setup protocol")
}

// The helper is already isolated and cancellable. Opening redirects here preserves
// source order; no second plan or pathname allocation is needed before exec.
pub(super) fn apply(mut arguments: &[CString], cwd: impl AsFd) -> io::Result<&[CString]> {
    fchdir(cwd)?;
    while let Some((tag, rest)) = arguments.split_first() {
        arguments = rest;
        match tag.to_bytes() {
            b"--" if !rest.is_empty() => return Ok(rest),
            b"2>&1" => dup2_stderr(stdout())?,
            b"cwd" | b"<" | b">" | b">>" | b"2>" | b"2>>" => {
                let (path, rest) = arguments.split_first().ok_or_else(invalid)?;
                arguments = rest;
                match tag.to_bytes() {
                    b"cwd" => fchdir(open(
                        path.as_c_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                        Mode::empty(),
                    )?)?,
                    b"<" => dup2_stdin(open(
                        path.as_c_str(),
                        OFlags::RDONLY | OFlags::NOCTTY | OFlags::CLOEXEC,
                        Mode::empty(),
                    )?)?,
                    tag => {
                        let output = open(
                            path.as_c_str(),
                            OFlags::WRONLY
                                | OFlags::NOCTTY
                                | OFlags::CREATE
                                | OFlags::CLOEXEC
                                | if tag.ends_with(b">>") {
                                    OFlags::APPEND
                                } else {
                                    OFlags::TRUNC
                                },
                            Mode::from_raw_mode(0o666),
                        )?;
                        if tag[0] == b'2' {
                            dup2_stderr(output)?;
                        } else {
                            dup2_stdout(output)?;
                        }
                    }
                }
            }
            _ => return Err(invalid()),
        }
    }
    Err(invalid())
}
