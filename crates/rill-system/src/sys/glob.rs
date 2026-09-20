//! Explicit POSIX globbing with native path bytes and deterministic byte ordering.
use std::{
    ffi::CStr,
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};
struct Matches(libc::glob_t);
impl Drop for Matches {
    fn drop(&mut self) {
        // SAFETY: glob initialized this result; this guard is its only owner.
        unsafe {
            libc::globfree(&raw mut self.0);
        }
    }
}
/// Expand a POSIX pattern in the calling process's directory.
/// Run in the isolated helper when resolving a session directory capability.
///
/// # Errors
/// Propagates traversal/allocation failures; no matches is an empty list.
pub fn expand(pattern: &CStr) -> io::Result<Vec<PathBuf>> {
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    // SAFETY: all-zero glob_t is valid initial storage; glob initializes its output members.
    let mut raw = unsafe { std::mem::zeroed::<libc::glob_t>() };
    // SAFETY: input is NUL terminated; raw is valid writable storage; no callback is installed.
    let result = unsafe {
        libc::glob(
            pattern.as_ptr(),
            libc::GLOB_ERR | libc::GLOB_NOSORT,
            None,
            &raw mut raw,
        )
    };
    let matches = Matches(raw);
    match result {
        0 => {}
        libc::GLOB_NOMATCH => return Ok(Vec::new()),
        libc::GLOB_NOSPACE => return Err(io::Error::other("glob exceeded available memory")),
        _ => return Err(io::Error::other("glob could not traverse the pattern")),
    }
    let mut paths = Vec::with_capacity(matches.0.gl_pathc);
    for index in 0..matches.0.gl_pathc {
        // SAFETY: successful glob owns gl_pathc initialized, NUL-terminated path pointers.
        let bytes = unsafe { CStr::from_ptr(*matches.0.gl_pathv.add(index)) }.to_bytes();
        paths.push(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())));
    }
    paths.sort_unstable_by(|left, right| {
        left.as_os_str()
            .as_bytes()
            .cmp(right.as_os_str().as_bytes())
    });
    Ok(paths)
}
