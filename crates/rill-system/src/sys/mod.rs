//! Narrow POSIX spawn boundary; child-side Rust runs only after a fresh exec.
use rustix::process::Pid;
use std::{
    ffi::{CStr, CString},
    io,
    mem::MaybeUninit,
    os::fd::{AsRawFd, BorrowedFd},
};

struct Actions(libc::posix_spawn_file_actions_t);
impl Actions {
    fn new() -> io::Result<Self> {
        let mut raw = MaybeUninit::uninit();
        // SAFETY: POSIX initializes the out-parameter on success.
        check(unsafe { libc::posix_spawn_file_actions_init(raw.as_mut_ptr()) })?;
        // SAFETY: initialization above succeeded.
        Ok(Self(unsafe { raw.assume_init() }))
    }
    fn duplicate(&mut self, source: BorrowedFd<'_>, target: i32) -> io::Result<()> {
        // SAFETY: the action list is initialized; the borrowed source stays open through spawn.
        check(unsafe {
            libc::posix_spawn_file_actions_adddup2(&raw mut self.0, source.as_raw_fd(), target)
        })
    }
}
impl Drop for Actions {
    fn drop(&mut self) {
        // SAFETY: this object exclusively owns an initialized action list.
        unsafe {
            libc::posix_spawn_file_actions_destroy(&raw mut self.0);
        }
    }
}
struct Attributes(libc::posix_spawnattr_t);
impl Attributes {
    fn new(group: Option<Pid>) -> io::Result<Self> {
        let mut raw = MaybeUninit::uninit();
        // SAFETY: POSIX initializes the out-parameter on success.
        check(unsafe { libc::posix_spawnattr_init(raw.as_mut_ptr()) })?;
        // SAFETY: initialization above succeeded; Drop handles later failures.
        let mut attr = Self(unsafe { raw.assume_init() });
        let flags = libc::POSIX_SPAWN_SETPGROUP
            | libc::POSIX_SPAWN_SETSIGDEF
            | libc::POSIX_SPAWN_SETSIGMASK;
        #[cfg(target_os = "macos")]
        let flags = flags | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
        let mut empty = MaybeUninit::uninit();
        let mut defaults = MaybeUninit::uninit();
        // SAFETY: these functions initialize signal sets before the attribute calls read them.
        unsafe {
            libc::sigemptyset(empty.as_mut_ptr());
            libc::sigfillset(defaults.as_mut_ptr());
            check(libc::posix_spawnattr_setsigmask(
                &raw mut attr.0,
                empty.as_ptr(),
            ))?;
            check(libc::posix_spawnattr_setsigdefault(
                &raw mut attr.0,
                defaults.as_ptr(),
            ))?;
            check(libc::posix_spawnattr_setpgroup(
                &raw mut attr.0,
                Pid::as_raw(group),
            ))?;
            check(libc::posix_spawnattr_setflags(
                &raw mut attr.0,
                i16::try_from(flags).expect("POSIX spawn flags fit c_short"),
            ))?;
        }
        Ok(attr)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: this object exclusively owns initialized attributes.
        unsafe {
            libc::posix_spawnattr_destroy(&raw mut self.0);
        }
    }
}

pub fn spawn(
    program: &CStr,
    argv: &[CString],
    environment: &[CString],
    duplicates: &[(BorrowedFd<'_>, i32)],
    group: Option<Pid>,
) -> io::Result<Pid> {
    // dup2(fd, fd) leaves CLOEXEC intact on Darwin. Use distinct owned sources
    // so the standard dup2 action establishes inheritance on both platforms.
    let inherited = duplicates
        .iter()
        .map(|(source, target)| {
            (source.as_raw_fd() == *target)
                .then(|| rustix::io::fcntl_dupfd_cloexec(*source, 3))
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut actions = Actions::new()?;
    for ((source, target), inherited) in duplicates.iter().zip(&inherited) {
        actions.duplicate(
            inherited.as_ref().map_or(*source, std::os::fd::AsFd::as_fd),
            *target,
        )?;
    }
    let attributes = Attributes::new(group)?;
    let argv = pointers(argv);
    let environment = pointers(environment);
    let mut pid = 0;
    // SAFETY: strings and terminated pointer arrays remain alive; spawn does not modify them.
    // Action/attribute owners and all borrowed descriptors remain valid until the call returns.
    check(unsafe {
        libc::posix_spawn(
            &raw mut pid,
            program.as_ptr(),
            &raw const actions.0,
            &raw const attributes.0,
            argv.as_ptr(),
            environment.as_ptr(),
        )
    })?;
    Pid::from_raw(pid).ok_or_else(|| io::Error::other("spawn returned an invalid process id"))
}
fn pointers(strings: &[CString]) -> Vec<*mut libc::c_char> {
    strings
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect()
}
fn check(code: i32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code))
    }
}

// The fresh helper already inherited the launch snapshot. execv preserves it;
// unlike execvp, it cannot invoke a shell for an unrecognized executable format.
pub fn exec(program: &CStr, argv: &[CString]) -> io::Error {
    let argv = pointers(argv);
    // SAFETY: valid C strings and a terminated pointer array stay alive until exec or failure.
    unsafe {
        libc::execv(program.as_ptr(), argv.as_ptr().cast());
    }
    io::Error::last_os_error()
}

/// The coordinator creates pipes and spawns children serially, with no await between
/// Apple's pipe and fcntl calls. No other Rill thread launches subprocesses.
pub fn pipe() -> io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    #[cfg(target_os = "linux")]
    {
        Ok(rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?)
    }
    #[cfg(target_os = "macos")]
    {
        let (read, write) = rustix::pipe::pipe()?;
        rustix::io::fcntl_setfd(&read, rustix::io::FdFlags::CLOEXEC)?;
        rustix::io::fcntl_setfd(&write, rustix::io::FdFlags::CLOEXEC)?;
        Ok((read, write))
    }
}

/// Restore executable signal and descriptor conventions in the isolated helper.
pub fn prepare_exec() -> io::Result<()> {
    // SAFETY: SIG_DFL is valid; the helper is isolated and single-threaded. Rust startup
    // ignores SIGPIPE, so spawn attributes alone cannot restore the final target.
    if unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) } == libc::SIG_ERR {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    {
        // libc exposes the bitmask as unsigned but close_range takes a C int.
        let flags = libc::CLOSE_RANGE_CLOEXEC.cast_signed();
        // SAFETY: CLOEXEC changes flags without closing live Rust-owned descriptors.
        if unsafe { libc::close_range(3, u32::MAX, flags) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // macOS uses POSIX_SPAWN_CLOEXEC_DEFAULT and explicit inheritance actions.
    Ok(())
}

pub fn ignore_terminal_output_signal() -> io::Result<()> {
    // SAFETY: SIG_IGN is a valid disposition. Child spawn resets inherited dispositions.
    if unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) } == libc::SIG_ERR {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub mod glob;
