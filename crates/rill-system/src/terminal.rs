//! Controlling-terminal ownership is restored before the editor can render again.
pub use crate::input::Input as Reader;
use rustix::{
    fs::{Mode, OFlags, open},
    process::{Pid, getpgrp},
    termios::{OptionalActions, Termios, tcgetattr, tcgetpgrp, tcsetattr, tcsetpgrp},
};
use std::{
    fs::File,
    io::{self, Write},
};

pub struct Terminal {
    fd: File,
    shell: Pid,
    attributes: Termios,
}
impl Terminal {
    /// Acquire the controlling terminal while the shell owns its foreground group.
    ///
    /// # Errors
    /// Rejects missing terminals and background invocation.
    pub fn open() -> io::Result<Self> {
        let fd: File = open("/dev/tty", OFlags::RDWR | OFlags::CLOEXEC, Mode::empty())?.into();
        let shell = getpgrp();
        if tcgetpgrp(&fd)? != shell {
            return Err(io::Error::other("shell is not in the foreground"));
        }
        let attributes = tcgetattr(&fd)?;
        crate::sys::ignore_terminal_output_signal()?;
        Ok(Self {
            fd,
            shell,
            attributes,
        })
    }
    /// Open an independent cancellable terminal reader; inherited stdin flags stay intact.
    /// The reader uses POSIX poll because `/dev/tty` is not a kqueue source on macOS.
    ///
    /// # Errors
    /// Reports terminal or cancellation-pipe failures.
    pub fn reader(&self) -> io::Result<Reader> {
        Reader::new(open(
            "/dev/tty",
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?)
    }
    /// Whether standard streams belong to the shell's foreground terminal group.
    #[must_use]
    pub fn owns_standard_streams(&self) -> bool {
        tcgetpgrp(io::stdin()).is_ok_and(|group| group == self.shell)
            && tcgetpgrp(io::stdout()).is_ok_and(|group| group == self.shell)
            && tcgetpgrp(io::stderr()).is_ok_and(|group| group == self.shell)
    }
    /// Whether the kernel provides a nonempty character-cell viewport.
    #[must_use]
    pub fn has_dimensions(&self) -> bool {
        rustix::termios::tcgetwinsize(&self.fd).is_ok_and(|size| size.ws_row > 0 && size.ws_col > 0)
    }
    /// Current output width, with a conservative fallback for terminals without dimensions.
    #[must_use]
    pub fn columns(&self) -> usize {
        rustix::termios::tcgetwinsize(&self.fd).map_or(80, |size| usize::from(size.ws_col).max(1))
    }
    /// Write editor text independently of redirected standard streams.
    ///
    /// # Errors
    /// Reports a terminal write failure.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        (&self.fd).write_all(bytes)
    }
    /// Discard the canonical line on signal cancellation, including externally sent SIGINT.
    ///
    /// # Errors
    /// Reports terminal queue or output failures.
    pub fn cancel_input(&self) -> io::Result<()> {
        rustix::termios::tcflush(&self.fd, rustix::termios::QueueSelector::IFlush)?;
        self.finish_line()
    }
    /// Finish an interrupted execution line before release callbacks write to the terminal.
    ///
    /// # Errors
    /// Reports a terminal write failure; callers must still complete resource cleanup.
    pub fn finish_line(&self) -> io::Result<()> {
        (&self.fd).write_all(b"\r\n")
    }
    /// Transfer foreground ownership to an established process group.
    ///
    /// # Errors
    /// Reports terminal ownership errors without discarding restoration state.
    pub fn handoff(&self, group: Pid) -> io::Result<()> {
        Ok(tcsetpgrp(&self.fd, group)?)
    }
    /// Capture a stopped foreground job's modes before restoring the shell baseline.
    ///
    /// # Errors
    /// Reports terminal attribute access failure.
    pub fn modes(&self) -> io::Result<Termios> {
        Ok(tcgetattr(&self.fd)?)
    }
    /// Restore a retained job's modes after transferring foreground ownership.
    ///
    /// # Errors
    /// Reports terminal attribute update failure.
    pub fn set_modes(&self, modes: &Termios) -> io::Result<()> {
        Ok(tcsetattr(&self.fd, OptionalActions::Now, modes)?)
    }
    /// Reclaim the terminal and restore the shell's cooked-mode baseline.
    ///
    /// # Errors
    /// Reports ownership or terminal-mode failures.
    pub fn restore(&self) -> io::Result<()> {
        tcsetpgrp(&self.fd, self.shell)?;
        Ok(tcsetattr(&self.fd, OptionalActions::Now, &self.attributes)?)
    }
    /// Suspend editing after its reader has stopped and all raw modes have been disabled.
    /// The parent shell resumes this group with SIGCONT; background resumes must wait
    /// for foreground ownership before any editor can enable raw input again.
    ///
    /// # Errors
    /// Reports terminal or signal failures. Background resumes remain stopped.
    pub fn suspend(&self) -> io::Result<()> {
        use rustix::process::{Signal, kill_process_group};
        self.restore()?;
        self.finish_line()?;
        // SIGTSTP is handled by the coordinator during evaluation. SIGSTOP makes this
        // explicit editor handoff independent of that process-wide signal handler.
        kill_process_group(self.shell, Signal::STOP)?;
        while tcgetpgrp(&self.fd)? != self.shell {
            // STOP also works for an orphaned process group, where TTIN is discarded.
            // Each later CONT rechecks ownership before the editor touches the terminal.
            kill_process_group(self.shell, Signal::STOP)?;
        }
        self.restore()
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
