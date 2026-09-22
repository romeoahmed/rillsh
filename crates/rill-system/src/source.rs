//! Descriptor-relative source loading keeps imports stable across directory renames.
use rustix::fs::{Mode, OFlags, open, openat};
use std::{
    fs::File,
    io::{self, Read},
    os::fd::AsFd,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// Stable identity of an opened file, including aliases through hard links.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Identity {
    device: u64,
    inode: u64,
}

/// An owned directory capability, independent of process-wide cwd changes.
#[derive(Debug)]
pub struct Directory(File);

impl Directory {
    /// Duplicate this directory capability without looking up its pathname again.
    ///
    /// # Errors
    /// Reports descriptor duplication failure.
    pub fn try_clone(&self) -> io::Result<Self> {
        self.0.try_clone().map(Self)
    }
    /// Open the base directory used for relative paths.
    ///
    /// # Errors
    /// Returns the OS error when the path cannot be opened as a directory.
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(
            open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?
            .into(),
        ))
    }

    /// Open a child directory relative to this capability.
    ///
    /// # Errors
    /// Reports path lookup or descriptor errors without changing the parent.
    pub fn at(&self, path: &Path) -> io::Result<Self> {
        Self::relative(&self.0, path)
    }
    /// Open a directory relative to an owned or borrowed directory descriptor.
    ///
    /// # Errors
    /// Rejects a non-directory destination or an inaccessible path.
    pub fn relative(fd: impl AsFd, path: &Path) -> io::Result<Self> {
        Ok(Self(
            openat(
                fd,
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?
            .into(),
        ))
    }
    /// Resolve the current physical name of this open directory.
    ///
    /// # Errors
    /// Reports a removed or inaccessible directory name.
    pub fn path(&self) -> io::Result<PathBuf> {
        directory_path(&self.0)
    }
    /// Transfer the directory descriptor into a launch snapshot.
    #[must_use]
    pub fn into_fd(self) -> std::os::fd::OwnedFd {
        self.0.into()
    }

    /// Open a regular source file and retain its parent for nested imports.
    /// Nonblocking open prevents a FIFO from hanging before the type check.
    ///
    /// # Errors
    /// Rejects non-regular files and reports path lookup or descriptor errors.
    pub fn source(&self, path: &Path) -> io::Result<SourceFile> {
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "source path must name a regular file",
            )
        })?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let directory = self.at(parent)?;
        let file: File = openat(
            &directory.0,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source must be a regular file",
            ));
        }
        Ok(SourceFile {
            identity: Identity {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            directory,
            file,
        })
    }
}

/// Opened and type-checked module input. Dropping it closes both descriptors.
#[derive(Debug)]
pub struct SourceFile {
    pub identity: Identity,
    pub directory: Directory,
    file: File,
}

impl SourceFile {
    /// Separate the source directory from the open file that pins its identity.
    #[must_use]
    pub fn into_parts(self) -> (Directory, File) {
        (self.directory, self.file)
    }

    /// Read UTF-8 source while enforcing the caller's byte budget.
    ///
    /// # Errors
    /// Reports invalid UTF-8, I/O failure, or a source exceeding `limit` bytes.
    pub fn read(&mut self, limit: usize) -> io::Result<String> {
        read_utf8(&mut self.file, limit)
    }
}

/// Read bounded UTF-8 input, checking the byte limit before decoding.
///
/// # Errors
/// Reports I/O failure, oversized input or invalid UTF-8. An oversized input is always
/// `FileTooLarge`, even when the bounded read ends inside a multibyte scalar.
pub fn read_utf8(input: impl Read, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    input
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!("input exceeds the {limit}-byte limit"),
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Resolve a physical directory name without changing process-global cwd.
///
/// # Errors
/// Reports a removed directory or unavailable native descriptor path lookup.
pub fn directory_path(fd: impl AsFd) -> io::Result<PathBuf> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    if rustix::fs::fstat(fd.as_fd())?.st_nlink == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "working directory was removed",
        ));
    }
    #[cfg(target_os = "macos")]
    let path = rustix::fs::getpath(fd)?;
    #[cfg(target_os = "linux")]
    let path = {
        use std::os::fd::AsRawFd;
        rustix::fs::readlink(
            format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd()),
            Vec::new(),
        )?
    };
    Ok(PathBuf::from(OsString::from_vec(path.into_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_limits_precede_utf8_validation_without_rejecting_exact_limits() {
        assert_eq!(read_utf8(b"".as_slice(), 0).unwrap(), "");
        assert_eq!(read_utf8("\u{e9}".as_bytes(), 2).unwrap(), "\u{e9}");
        for (bytes, limit) in [(b"x".as_slice(), 0), ("\u{e9}\u{e9}".as_bytes(), 2)] {
            assert_eq!(
                read_utf8(bytes, limit).unwrap_err().kind(),
                io::ErrorKind::FileTooLarge
            );
        }
        assert_eq!(
            read_utf8(b"\xff".as_slice(), 1).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
