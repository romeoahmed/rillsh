//! A fresh executable waits for launch commit, then execs the target without a fork callback.
pub(crate) mod setup;
use crate::sys;
use rustix::io::{FdFlags, fcntl_setfd, read, write};
use std::{
    ffi::{CString, OsString},
    io,
    os::{
        fd::{AsFd, FromRawFd, OwnedFd},
        unix::ffi::{OsStrExt, OsStringExt},
    },
};

/// Run the private launch entry point when selected by the application.
///
/// Arguments are gate fd, status fd, cwd fd, stage index, setup instructions,
/// a `--` separator, then the executable and its arguments.
///
/// # Safety
/// Call only at single-threaded process startup. The three inherited protocol descriptors
/// must have no existing Rust owners; this entry point takes exclusive ownership of them.
///
/// # Errors
/// Rejects malformed descriptor arguments before taking ownership. Setup or exec
/// failures are reported on the status channel and return the helper exit code.
pub unsafe fn run(arguments: impl IntoIterator<Item = OsString>) -> io::Result<u8> {
    let mut protocol = arguments.into_iter();
    let mut number = || {
        protocol
            .next()
            .and_then(|s| s.into_string().ok())
            .and_then(|s| s.parse::<i32>().ok())
            .filter(|n| *n >= 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid launch protocol argument",
                )
            })
    };
    let gate = number()?;
    let status = number()?;
    let cwd = number()?;
    let stage = number()?;
    if gate < 3 || status < 3 || cwd < 3 || gate == status || gate == cwd || status == cwd {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "launch descriptors must be distinct nonstandard descriptors",
        ));
    }
    for fd in [gate, status, cwd] {
        // SAFETY: fcntl accepts arbitrary integers and reports EBADF without creating a Rust borrow.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: all three descriptors were validated, are distinct, and are exclusively transferred
    // to this single-threaded helper entry point; no other Rust object owns them.
    let (gate, status, cwd) = unsafe {
        (
            OwnedFd::from_raw_fd(gate),
            OwnedFd::from_raw_fd(status),
            OwnedFd::from_raw_fd(cwd),
        )
    };
    let argv = protocol
        .map(|s| {
            CString::new(s.into_vec())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let result = execute(&gate, &status, &cwd, stage, &argv);
    let code = result.raw_os_error().unwrap_or(libc::EIO);
    let message = [stage.to_le_bytes(), code.to_le_bytes()].concat();
    let mut remaining = message.as_slice();
    while !remaining.is_empty() {
        match write(&status, remaining) {
            Ok(0) => break,
            Ok(count) => remaining = &remaining[count..],
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => break,
        }
    }
    Ok(127)
}
fn execute(
    gate: &OwnedFd,
    status: &OwnedFd,
    cwd: &OwnedFd,
    stage: i32,
    argv: &[CString],
) -> io::Error {
    if let Err(error) = sys::prepare_exec() {
        return error;
    }
    if let Err(error) = fcntl_setfd(status, FdFlags::CLOEXEC)
        .and_then(|()| fcntl_setfd(cwd, FdFlags::CLOEXEC))
        .and_then(|()| fcntl_setfd(gate, FdFlags::CLOEXEC))
    {
        return error.into();
    }
    let argv = match setup::apply(argv, cwd) {
        Ok(argv) => argv,
        Err(error) => return error,
    };
    let program = &argv[0];
    // Readiness also confirms ordered redirects succeeded. No target runs until
    // every helper has reached this barrier; blocked opens remain cancellable.
    let ready = [stage.to_le_bytes(), 0_i32.to_le_bytes()].concat();
    loop {
        match write(status, &ready) {
            Ok(count) if count == ready.len() => break,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return error.into(),
            Ok(_) => return io::Error::other("incomplete launch readiness record"),
        }
    }
    let mut byte = [0];
    loop {
        match read(gate.as_fd(), &mut byte) {
            Ok(1) => break,
            Err(rustix::io::Errno::INTR) => {}
            Ok(_) => return io::Error::new(io::ErrorKind::BrokenPipe, "launch was cancelled"),
            Err(error) => return error.into(),
        }
    }
    if program.to_bytes().contains(&b'/') {
        return sys::exec(program, argv);
    }
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let mut denied = false;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(std::ffi::OsStr::from_bytes(program.to_bytes()));
        let Ok(candidate) = CString::new(candidate.as_os_str().as_bytes()) else {
            continue;
        };
        let error = sys::exec(&candidate, argv);
        match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOTDIR) => {}
            Some(libc::EACCES) => denied = true,
            _ => return error,
        }
    }
    io::Error::from_raw_os_error(if denied { libc::EACCES } else { libc::ENOENT })
}
