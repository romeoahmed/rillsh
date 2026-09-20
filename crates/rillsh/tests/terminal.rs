//! Assert rendered terminal behavior through a real controlling PTY.
use rustix::{
    event::{PollFd, PollFlags, poll},
    fs::{Mode, OFlags, open},
    io::{Errno, read, write},
    pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt},
    termios::{Winsize, tcsetwinsize},
};
use std::{
    io,
    os::{fd::OwnedFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Replies(Vec<Vec<u8>>);
impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        _: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        code: char,
    ) {
        if code == 'n' && params == [&[6]] {
            let (row, column) = screen.cursor_position();
            self.0
                .push(format!("\x1b[{};{}R", row + 1, column + 1).into_bytes());
        }
    }
}
struct Terminal {
    master: Option<OwnedFd>,
    slave: Option<OwnedFd>,
    child: Child,
    parser: vt100::Parser<Replies>,
    home: tempfile::TempDir,
    output: Vec<u8>,
}
impl Terminal {
    fn new() -> io::Result<Self> {
        Self::with_state(|_| Ok(()))
    }
    fn with_state(setup: impl FnOnce(&std::path::Path) -> io::Result<()>) -> io::Result<Self> {
        Self::configured(|home, _, _| setup(home))
    }
    fn configured(
        configure: impl FnOnce(&std::path::Path, &mut Command, &OwnedFd) -> io::Result<()>,
    ) -> io::Result<Self> {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        grantpt(&master)?;
        unlockpt(&master)?;
        let slave = open(
            ptsname(&master, Vec::new())?,
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        tcsetwinsize(
            &slave,
            Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )?;
        let home = tempfile::tempdir()?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_rillsh"));
        command
            .arg("-i")
            .env_remove("COLORTERM")
            .env_remove("NO_COLOR")
            .env("TERM", "rill-test-256color")
            .env("XDG_STATE_HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave.try_clone()?));
        configure(home.path(), &mut command, &slave)?;
        // Keep the terminal alive across exec when every standard stream is redirected.
        let retained_slave = slave.try_clone()?;
        // SAFETY: this callback performs only async-signal-safe POSIX calls, without allocation.
        // The captured slave remains open even when standard streams are redirected.
        unsafe {
            command.pre_exec(move || {
                rustix::process::setsid()?;
                rustix::process::ioctl_tiocsctty(&slave)?;
                Ok(())
            });
        }
        Ok(Self {
            master: Some(master),
            slave: Some(retained_slave),
            child: command.spawn()?,
            parser: vt100::Parser::new_with_callbacks(24, 80, 0, Replies::default()),
            home,
            output: Vec::new(),
        })
    }
    fn send(&self, bytes: &[u8]) -> io::Result<()> {
        let mut bytes = bytes;
        while !bytes.is_empty() {
            match write(self.master.as_ref().unwrap(), bytes) {
                Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                Ok(count) => bytes = &bytes[count..],
                Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
    fn until(&mut self, predicate: impl Fn(&vt100::Screen) -> bool) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !predicate(self.parser.screen()) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, self.parser.screen().contents())
                })?;
            let timeout = remaining.try_into().map_err(io::Error::other)?;
            let mut descriptors = [PollFd::new(self.master.as_ref().unwrap(), PollFlags::IN)];
            if poll(&mut descriptors, Some(&timeout))? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    self.parser.screen().contents(),
                ));
            }
            let mut bytes = [0; 4096];
            let count = read(self.master.as_ref().unwrap(), &mut bytes)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    self.parser.screen().contents(),
                ));
            }
            self.output.extend_from_slice(&bytes[..count]);
            self.parser.process(&bytes[..count]);
            for reply in std::mem::take(&mut self.parser.callbacks_mut().0) {
                self.send(&reply)?;
            }
        }
        Ok(())
    }
    fn wait_for_exit(&mut self) -> io::Result<()> {
        self.slave.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait()? {
                assert!(status.success());
                break;
            }
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .try_into()
                .map_err(io::Error::other)?;
            let mut descriptors = [PollFd::new(
                self.master.as_ref().unwrap(),
                PollFlags::IN | PollFlags::HUP,
            )];
            if poll(&mut descriptors, Some(&timeout))? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "shell did not exit after Ctrl+D",
                ));
            }
            let mut bytes = [0; 4096];
            match read(self.master.as_ref().unwrap(), &mut bytes) {
                Ok(0) | Err(Errno::IO) => {
                    assert!(self.child.wait()?.success());
                    break;
                }
                Ok(count) => {
                    self.output.extend_from_slice(&bytes[..count]);
                    self.parser.process(&bytes[..count]);
                }
                Err(error) => return Err(error.into()),
            }
            for reply in std::mem::take(&mut self.parser.callbacks_mut().0) {
                self.send(&reply)?;
            }
        }
        Ok(())
    }
    fn prompt_after(&mut self, previous_row: u16) -> io::Result<u16> {
        self.until(|screen| {
            let (row, column) = screen.cursor_position();
            row > previous_row
                && column == 6
                && screen.contents().trim_end().lines().last() == Some("rill>")
        })?;
        Ok(self.parser.screen().cursor_position().0)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.master.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn history_search_reports_misses_and_accepts_matches_without_execution() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill>"))?;
    terminal.send(b"12345 + 1\r")?;
    terminal.until(|screen| {
        screen.contents().lines().any(|line| line == "12346")
            && screen.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x12missing-history-entry")?;
    terminal.until(|screen| screen.contents().contains("history (no match)"))?;
    terminal.send(b"\x03")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill>"))?;
    terminal.send(b"\x1212345")?;
    terminal.until(|screen| screen.contents().contains("history '12345'"))?;
    terminal.send(b"\r")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill> 12345 + 1"))?;
    terminal.send(b"\x03")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill>"))?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn startup_selects_the_default_explicit_file_or_no_file() -> io::Result<()> {
    for (mode, expected) in [
        ("default", Some("default config")),
        ("explicit", Some("selected config")),
        ("disabled", None),
    ] {
        let mut terminal = Terminal::configured(|home, command, _| {
            let config = home.join("config/rillsh");
            std::fs::create_dir_all(&config)?;
            std::fs::write(config.join("init.rill"), "print \"default config\"")?;
            let selected = home.join("selected.rill");
            std::fs::write(&selected, "print \"selected config\"")?;
            match mode {
                "explicit" => {
                    command.arg("--config").arg(selected);
                }
                "disabled" => {
                    command.arg("--no-config");
                }
                _ => {}
            }
            Ok(())
        })?;
        terminal.until(|screen| screen.contents().trim_end().ends_with("rill>"))?;
        let output = terminal.parser.screen().contents();
        let startup: Vec<_> = output
            .lines()
            .filter(|line| matches!(*line, "default config" | "selected config"))
            .collect();
        assert_eq!(startup, expected.into_iter().collect::<Vec<_>>());
        terminal.send(b"\x04")?;
        terminal.wait_for_exit()?;
    }
    Ok(())
}

#[test]
fn editor_suspension_restores_modes_and_retains_the_unsubmitted_entry() -> io::Result<()> {
    use rustix::process::{Pid, Signal, WaitOptions, kill_process, waitpid};
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"print \"after resume\"")?;
    terminal.until(|screen| screen.contents().contains("after resume"))?;
    let pid = Pid::from_raw(i32::try_from(terminal.child.id()).unwrap()).unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let waiter = std::thread::spawn(move || {
        let _ = sender.send(waitpid(Some(pid), WaitOptions::UNTRACED));
    });
    terminal.send(b"\x1a")?;
    let (_, status) = receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(io::Error::other)??
        .expect("stopped child");
    waiter.join().unwrap();
    assert!(status.stopped());
    let modes = rustix::termios::tcgetattr(terminal.slave.as_ref().unwrap())?;
    assert!(
        modes
            .local_modes
            .contains(rustix::termios::LocalModes::ICANON)
    );
    terminal.parser = vt100::Parser::new_with_callbacks(24, 80, 0, Replies::default());
    kill_process(pid, Signal::CONT)?;
    terminal.until(|screen| screen.contents().contains("rill> print \"after resume\""))?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen.contents().lines().any(|line| line == "after resume")
            && screen.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn filesystem_completion_inserts_quoted_source_without_executing_it() -> io::Result<()> {
    let mut terminal = Terminal::configured(|home, command, _| {
        command.current_dir(home);
        std::fs::write(home.join("completion $(ignored)"), b"")?;
        std::fs::write(home.join("completion second"), b"")
    })?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"^printf \"%s\\n\" comp\t")?;
    terminal.until(|screen| {
        screen.contents().contains("completion second") && screen.contents().contains("Path")
    })?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .trim_end()
            .ends_with("\"completion $(ignored)\"")
    })?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "completion $(ignored)")
    );
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line == "completion $(ignored)")
            && screen.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn unavailable_history_keeps_editing_execution_and_recall_usable() -> io::Result<()> {
    let mut terminal =
        Terminal::with_state(|home| std::fs::write(home.join("rillsh"), b"not a directory"))?;
    terminal.until(|screen| {
        screen.contents().contains("persistent history unavailable")
            && screen.contents().trim_end().ends_with("rill>")
            && screen.cursor_position().1 == 6
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"40 + 2\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    terminal.prompt_after(row)?;
    terminal.send(b"\x1b[A")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill> 40 + 2"))?;
    terminal.send(b"\x03")?;
    terminal.until(|screen| {
        screen.contents().trim_end().ends_with("rill>") && screen.cursor_position().1 == 6
    })?;
    Ok(())
}

#[test]
fn cancellation_starts_one_fresh_prompt_and_discards_the_entry() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| {
        screen.contents().trim_end() == "rill>" && screen.cursor_position().1 == 6
    })?;
    let mut row = terminal.parser.screen().cursor_position().0;
    for input in [
        b"\x03".as_slice(),
        b"let stale = 99\x03",
        b"do {\rlet stale = 99\x03",
    ] {
        terminal.send(input)?;
        row = terminal.prompt_after(row)?;
        assert!(!terminal.parser.screen().contents().contains("^Crill>"));
    }
    terminal.send(b"40 + 2\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    row = terminal.prompt_after(row)?;
    terminal.send(b"let interrupted = 7")?;
    terminal.until(|screen| screen.contents().contains("let interrupted = 7"))?;
    let pid = rustix::process::Pid::from_raw(
        i32::try_from(terminal.child.id()).map_err(io::Error::other)?,
    )
    .ok_or_else(|| io::Error::other("invalid child PID"))?;
    rustix::process::kill_process(pid, rustix::process::Signal::INT)?;
    terminal.prompt_after(row)?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()?;
    Ok(())
}

#[test]
fn foreground_interrupt_finishes_cleanup_before_the_next_prompt() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(
        b"attempt { () => do { ^sh -c 'printf running; exec sleep 30'; print \"continued\" } }\r",
    )?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "running"))?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"\x03")?;
    terminal.prompt_after(row)?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "continued")
    );
    terminal.send(b"21 * 2\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    Ok(())
}

#[test]
fn background_completion_repaints_without_losing_partial_input() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    let gate = terminal.home.path().join("notification-gate");
    assert!(Command::new("mkfifo").arg(&gate).status()?.success());
    let source = format!(
        "let worker = start (job {{ ^sh -c 'read value < \"$1\"' sh {:?} }})\r",
        gate.to_str().unwrap()
    );
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(source.as_bytes())?;
    terminal.prompt_after(row)?;
    terminal.send(b"40 +")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill> 40 +"))?;
    let release = std::thread::spawn(move || std::fs::write(gate, b"ready\n"));
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "[job 1] completed")
            && screen.contents().trim_end().ends_with("rill> 40 +")
    })?;
    release.join().unwrap()?;
    terminal.send(b" 2\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    Ok(())
}

#[test]
fn returned_stream_displays_before_eof_and_then_publishes() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    let gate = terminal.home.path().join("display-gate");
    assert!(Command::new("mkfifo").arg(&gate).status()?.success());
    let source = format!(
        "let saved = 42; stream (job {{ ^sh -c 'printf \"first\\n\"; read token < \"$1\"; printf \"last\\n\"' display {:?} }}) |> lines\r",
        gate.to_str().unwrap()
    );
    terminal.send(source.as_bytes())?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "\"first\"")
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    let release = std::thread::spawn(move || std::fs::write(gate, b"ready\n"));
    terminal.prompt_after(row)?;
    release.join().unwrap()?;
    assert!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line.trim_end() == "\"last\"")
    );
    terminal.send(b"saved\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "42")
    })?;
    Ok(())
}

#[test]
fn bracketed_paste_inserts_multiline_source_without_executing_it() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"\x1b[200~print \"paste-one\"\nprint \"paste-two\"\x1b[201~")?;
    terminal.until(|screen| screen.contents().contains("print \"paste-two\""))?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| { matches!(line.trim_end(), "paste-one" | "paste-two") })
    );
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"\r")?;
    terminal.prompt_after(row)?;
    let contents = terminal.parser.screen().contents();
    let output: Vec<_> = contents
        .lines()
        .map(str::trim_end)
        .filter(|line| matches!(*line, "paste-one" | "paste-two"))
        .collect();
    assert_eq!(output, ["paste-one", "paste-two"]);
    Ok(())
}

#[test]
fn producer_cancellation_finishes_release_and_owned_io_before_the_next_prompt() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(br#"attempt { () => produce { acquire: { () => stream (job { ^sleep 30 }) }, step: { source => do { print "waiting"; collect_bytes source; Option.None } }, release: { state reason => print "released" } } |> collect }; print "continued""#)?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "waiting")
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"\x03")?;
    terminal.prompt_after(row)?;
    let contents = terminal.parser.screen().contents();
    assert_eq!(
        contents
            .lines()
            .filter(|line| line.trim_end() == "released")
            .count(),
        1,
        "{contents}"
    );
    assert!(!contents.lines().any(|line| line.trim_end() == "continued"));
    terminal.send(b"21 * 2\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "42")
    })?;
    Ok(())
}

#[test]
fn cancellation_reports_release_failure_without_hiding_the_next_prompt() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(br#"produce { acquire: { () => 0 }, step: { state => do { print "waiting"; let spin = rec { spin () => spin () }; spin () } }, release: { state reason => raise (error "ReleaseFailure" "cleanup diagnostic") } } |> collect"#)?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "waiting")
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"\x03")?;
    terminal.prompt_after(row)?;
    assert!(
        terminal
            .parser
            .screen()
            .contents()
            .contains("cleanup diagnostic")
    );
    Ok(())
}

#[test]
fn interrupts_during_release_do_not_cancel_the_next_entry() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    let gate = terminal.home.path().join("release-gate");
    assert!(Command::new("mkfifo").arg(&gate).status()?.success());
    let source = format!(
        r#"produce {{ acquire: {{ () => 0 }}, step: {{ state => do {{ print "waiting"; let spin = rec {{ spin () => spin () }}; spin () }} }}, release: {{ state reason => do {{ ^sh -c 'printf "releasing\n"; read token < "$1"' release {:?}; print "released" }} }} }} |> collect"#,
        gate.to_str().unwrap()
    );
    terminal.send(source.as_bytes())?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "waiting")
    })?;
    terminal.send(b"\x03")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "releasing")
    })?;
    let pid = rustix::process::Pid::from_raw(
        i32::try_from(terminal.child.id()).map_err(io::Error::other)?,
    )
    .ok_or_else(|| io::Error::other("invalid child PID"))?;
    rustix::process::kill_process(pid, rustix::process::Signal::INT)?;
    let row = terminal.parser.screen().cursor_position().0;
    let release = std::thread::spawn(move || std::fs::write(gate, b"ready\n"));
    let row = terminal.prompt_after(row)?;
    release.join().unwrap()?;
    terminal.send(b"21 * 2\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .lines()
            .any(|line| line.trim_end() == "42")
    })?;
    terminal.prompt_after(row)?;
    let contents = terminal.parser.screen().contents();
    assert_eq!(
        contents
            .lines()
            .filter(|line| line.trim_end() == "released")
            .count(),
        1
    );
    assert_eq!(
        contents
            .lines()
            .filter(|line| line.trim_end() == "rill>")
            .count(),
        1,
        "{contents}"
    );
    Ok(())
}

#[test]
fn blocked_launch_opens_are_cancellable_in_every_execution_mode() -> io::Result<()> {
    for mode in ["run", "capture", "stream", "start", "through"] {
        let mut terminal = Terminal::new()?;
        terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
        let gate = terminal.home.path().join("launch-gate");
        let marker = terminal.home.path().join("setup-started");
        assert!(Command::new("mkfifo").arg(&gate).status()?.success());
        let job = if mode == "through" {
            format!(
                "job {{ ^cat | ^cat 2> {:?} > {:?} | ^cat }}",
                marker.to_str().unwrap(),
                gate.to_str().unwrap()
            )
        } else {
            format!(
                "job {{ ^printf never > {:?} < {:?} }}",
                marker.to_str().unwrap(),
                gate.to_str().unwrap()
            )
        };
        let operation = match mode {
            "through" => {
                format!("chunks (encode_utf8 \"input\") |> through ({job}) |> collect_bytes")
            }
            "stream" => format!("stream ({job}) |> collect_bytes"),
            mode => format!("{mode} ({job})"),
        };
        terminal.send(
            format!("print \"launching\"; attempt {{ () => {operation} }}; print \"continued\"\r")
                .as_bytes(),
        )?;
        terminal.until(|screen| {
            screen
                .contents()
                .lines()
                .any(|line| line.trim_end() == "launching")
        })?;
        // File creation precedes the blocking open. Observe that effect instead of
        // assuming the helper has reached it after an arbitrary delay.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, mode));
            }
            std::thread::yield_now();
        }
        let pid = rustix::process::Pid::from_raw(
            i32::try_from(terminal.child.id()).map_err(io::Error::other)?,
        )
        .ok_or_else(|| io::Error::other("invalid shell PID"))?;
        let row = terminal.parser.screen().cursor_position().0;
        if mode == "run" {
            while rustix::termios::tcgetpgrp(terminal.master.as_ref().unwrap())? == pid {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "foreground handoff",
                    ));
                }
                std::thread::yield_now();
            }
            terminal.send(b"\x03")?;
        } else {
            rustix::process::kill_process(pid, rustix::process::Signal::INT)?;
        }
        terminal.prompt_after(row)?;
        assert!(
            std::fs::read(&marker)?.is_empty(),
            "{mode} ran its target before setup committed"
        );
        assert!(
            !terminal
                .parser
                .screen()
                .contents()
                .lines()
                .any(|line| line.trim_end() == "continued"),
            "{mode} swallowed cancellation"
        );
        terminal.send(b"21 * 2\r")?;
        terminal.until(|screen| {
            screen
                .contents()
                .lines()
                .any(|line| line.trim_end() == "42")
        })?;
    }
    Ok(())
}

#[test]
fn plain_or_unusable_terminals_use_canonical_input_without_escape_sequences() -> io::Result<()> {
    for term in [None, Some(""), Some("dumb"), Some("xterm-256color")] {
        let mut terminal = Terminal::configured(|_, command, slave| {
            if let Some(term) = term {
                command.env("TERM", term);
            } else {
                command.env_remove("TERM");
            }
            if term == Some("xterm-256color") {
                tcsetwinsize(
                    slave,
                    Winsize {
                        ws_row: 0,
                        ws_col: 0,
                        ws_xpixel: 0,
                        ws_ypixel: 0,
                    },
                )?;
            }
            Ok(())
        })?;
        terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
        terminal.send(b"40 + 2\r")?;
        terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
        terminal.prompt_after(0)?;
        terminal.send(b"\x04")?;
        terminal.wait_for_exit()?;
        assert!(!terminal.output.contains(&0x1b), "{term:?}");
    }
    Ok(())
}

#[test]
fn explicit_interaction_uses_controlling_terminal_with_all_standard_streams_redirected()
-> io::Result<()> {
    let mut terminal = Terminal::configured(|home, command, _| {
        std::fs::write(home.join("stdin"), b"redirected input\n")?;
        let output = std::fs::File::create(home.join("output"))?;
        command
            .stdin(std::fs::File::open(home.join("stdin"))?)
            .stdout(output.try_clone()?)
            .stderr(output);
        Ok(())
    })?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"^cat; print \"redirected output\"; 42\r")?;
    terminal.prompt_after(0)?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()?;
    let output = std::fs::read_to_string(terminal.home.path().join("output"))?;
    assert!(
        output.contains("redirected input\nredirected output\n42\n"),
        "{output}"
    );
    assert!(!output.contains("rill>"));
    assert!(!output.contains('\x1b'));
    assert!(!terminal.output.contains(&0x1b));
    Ok(())
}

#[test]
fn canonical_input_cancels_whole_entries_and_diagnoses_incomplete_eof() -> io::Result<()> {
    let mut terminal = Terminal::configured(|_, command, _| {
        command.env("TERM", "dumb");
        Ok(())
    })?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"do {\r")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("..."))?;
    terminal.send(b"40 + 2\r")?;
    terminal.until(|screen| {
        screen.contents().contains("40 + 2") && screen.contents().trim_end().ends_with("...")
    })?;
    terminal.send(b"}\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    let mut row = terminal.prompt_after(0)?;
    terminal.send(b"do {\r")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("..."))?;
    terminal.send(b"print \"must not execute\"\r")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("..."))?;
    terminal.send(b"\x04")?;
    terminal.until(|screen| screen.contents().contains("ParseError"))?;
    row = terminal.prompt_after(row)?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "must not execute")
    );
    terminal.send(b"let stale = 99")?;
    terminal.until(|screen| screen.contents().contains("let stale = 99"))?;
    let pid = rustix::process::Pid::from_raw(
        i32::try_from(terminal.child.id()).map_err(io::Error::other)?,
    )
    .unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::INT)?;
    row = terminal.prompt_after(row)?;
    terminal.send(b"21 * 2\r")?;
    terminal.prompt_after(row)?;
    terminal.send(b"\xff\r")?;
    terminal.until(|screen| {
        screen.contents().contains("input must be UTF-8")
            && screen.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()?;
    assert!(!terminal.output.contains(&0x1b));
    Ok(())
}

#[test]
fn session_hints_change_color_depth_at_the_next_prompt() -> io::Result<()> {
    use rill_editor::profile::ColorDepth;
    for (term, colorterm, depth) in [
        ("xterm", "", ColorDepth::Ansi16),
        ("xterm-256color", "", ColorDepth::Ansi256),
        ("xterm-256color", "truecolor", ColorDepth::TrueColor),
    ] {
        let mut terminal = Terminal::configured(|_, command, _| {
            command.env("TERM", term).env("COLORTERM", colorterm);
            Ok(())
        })?;
        terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
        terminal.send(b"'color probe'")?;
        terminal.until(|screen| screen.contents().contains("'color probe'"))?;
        assert!(matches!(
            (
                depth,
                terminal.parser.screen().cell(0, 6).unwrap().fgcolor()
            ),
            (ColorDepth::Ansi16, vt100::Color::Idx(0..=15))
                | (ColorDepth::Ansi256, vt100::Color::Idx(16..=255))
                | (ColorDepth::TrueColor, vt100::Color::Rgb(..))
        ));
        terminal.send(b"\x03")?;
        let row = terminal.prompt_after(0)?;
        terminal.send(b"set_env \"NO_COLOR\" \"1\"\r")?;
        let row = terminal.prompt_after(row)?;
        terminal.send(b"'plain probe'")?;
        terminal.until(|screen| screen.contents().contains("'plain probe'"))?;
        assert_eq!(
            terminal.parser.screen().cell(row, 6).unwrap().fgcolor(),
            vt100::Color::Default
        );
        terminal.send(b"\x03")?;
        let row = terminal.prompt_after(row)?;
        terminal.send(b"set_env \"TERM\" \"dumb\"\r")?;
        let row = terminal.prompt_after(row)?;
        terminal.output.clear();
        terminal.send(b"21 * 2\r")?;
        terminal.prompt_after(row)?;
        terminal.send(b"\x04")?;
        terminal.wait_for_exit()?;
        assert!(!terminal.output.contains(&0x1b));
    }
    Ok(())
}

#[test]
fn canonical_interrupt_discards_multiline_input_and_releases_foreground_io() -> io::Result<()> {
    let mut terminal = Terminal::configured(|_, command, _| {
        command.env("TERM", "dumb");
        Ok(())
    })?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"do {\r")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("..."))?;
    terminal.send(b"let abandoned = 99\x03")?;
    let row = terminal.prompt_after(0)?;
    assert!(!terminal.parser.screen().contents().contains("^Crill>"));
    terminal.send(b"^sh -c 'printf running; exec sleep 30'\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "running"))?;
    terminal.send(b"\x03")?;
    let row = terminal.prompt_after(row)?;
    terminal.send(b"40 + 2\r")?;
    terminal.prompt_after(row)?;
    assert!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "42")
    );
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()?;
    assert!(!terminal.output.contains(&0x1b));
    Ok(())
}

#[test]
fn completion_acceptance_only_edits_and_metadata_refreshes_after_publication() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal
        .send(b"let completion_alpha = { () => print \"invoked\" }; let completion_beta = 2\r")?;
    let row = terminal.prompt_after(0)?;
    terminal.send(b"completion_\t")?;
    terminal.until(|screen| {
        screen.contents().contains("completion_alpha")
            && screen.contents().contains("completion_beta")
            && screen.contents().contains("Function")
    })?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .trim_end()
            .ends_with("rill> completion_alpha")
    })?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "invoked")
    );
    assert_eq!(terminal.parser.screen().cursor_position().0, row);
    terminal.send(b" ()\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "invoked"))?;
    let row = terminal.prompt_after(row)?;
    terminal.send(b"let completion_alpha = {nested: {answer: 42}}\r")?;
    let row = terminal.prompt_after(row)?;
    terminal.send(b"completion_alpha.nested.an\t")?;
    terminal.until(|screen| {
        screen.contents().contains("answer") && screen.contents().contains("Int")
    })?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .trim_end()
            .ends_with("rill> completion_alpha.nested.answer")
    })?;
    terminal.send(b"\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    terminal.prompt_after(row)?;
    Ok(())
}

#[test]
fn completion_escape_and_cursor_replacement_preserve_the_unsubmitted_entry() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b"let completion_alpha = 40; let completion_beta = 99\r")?;
    let row = terminal.prompt_after(0)?;
    terminal.send(b"completion_\t")?;
    terminal.until(|screen| {
        screen.contents().contains("completion_beta") && screen.contents().contains("Int")
    })?;
    terminal.send(b"\x1b")?;
    terminal.until(|screen| screen.contents().trim_end().ends_with("rill> completion_"))?;
    terminal.send(b"alpha + 2\x01")?;
    terminal.until(|screen| screen.cursor_position() == (row, 6))?;
    terminal.send(b"\x1b[C\x1b[C\x1b[C\x1b[C\t")?;
    terminal.until(|screen| {
        screen.contents().contains("completion_beta") && screen.contents().contains("Int")
    })?;
    terminal.send(b"\r")?;
    terminal.until(|screen| {
        screen
            .contents()
            .trim_end()
            .ends_with("rill> completion_alpha + 2")
    })?;
    terminal.send(b"\x05\r")?;
    terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
    let row = terminal.prompt_after(row)?;
    terminal.send(b"completion_\t")?;
    terminal.until(|screen| {
        screen.contents().contains("completion_beta") && screen.contents().contains("Int")
    })?;
    terminal.send(b"\x1b\r")?;
    terminal.until(|screen| screen.contents().contains("NameError"))?;
    terminal.prompt_after(row)?;
    Ok(())
}

#[test]
fn explicit_newline_and_submit_keys_preserve_whole_entry_validation() -> io::Result<()> {
    for newline in [b"\n".as_slice(), b"\x1b[Z"] {
        let mut terminal = Terminal::new()?;
        terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
        terminal.send(b"let answer = 42")?;
        terminal.send(newline)?;
        terminal.until(|screen| screen.contents().trim_end().ends_with("..."))?;
        terminal.send(b"answer\x1b\r")?;
        terminal.until(|screen| screen.contents().lines().any(|line| line == "42"))?;
        let row = terminal.prompt_after(0)?;
        terminal.send(b"print \"must not execute\"; do {\x1b\r")?;
        terminal.until(|screen| screen.contents().contains("ParseError"))?;
        terminal.prompt_after(row)?;
        assert!(
            !terminal
                .parser
                .screen()
                .contents()
                .lines()
                .any(|line| line == "must not execute")
        );
    }
    Ok(())
}

#[test]
fn leading_space_history_is_recallable_but_not_persisted() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(b" let private_entry = 42\r")?;
    let row = terminal.prompt_after(0)?;
    terminal.send(b"\x1b[A")?;
    terminal.until(|screen| {
        screen
            .contents()
            .trim_end()
            .ends_with("rill>  let private_entry = 42")
    })?;
    terminal.send(b"\x03")?;
    terminal.prompt_after(row)?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()?;
    let history = std::fs::read_to_string(terminal.home.path().join("rillsh/history"))?;
    assert!(!history.contains("private_entry"));
    Ok(())
}

#[test]
fn foreground_stop_retains_lexical_state_and_publishes_on_fg() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(b"let captured = 40\r")?;
    terminal.prompt_after(0)?;
    terminal.send(br"let saved = captured + 2; ^sh -c 'kill -STOP $$'; saved")?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"let handle = (jobs ())[0].handle; let captured = 99; let interim = 7\r")?;
    terminal.prompt_after(row)?;
    terminal.send(b"fg handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"saved + captured + interim\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "148") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn capture_resumes_without_relaunching_or_losing_partial_output() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br"let captured = capture (job { ^sh -c 'printf before; printf error >&2; kill -STOP $$; printf after' }); print (decode_utf8 captured.stdout)")?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"fg (jobs ())[0].handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "beforeafter")
            && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"print (decode_utf8 captured.stderr)\r")?;
    terminal.until(|s| s.contents().lines().any(|line| line == "error"))?;
    Ok(())
}

#[test]
fn stopped_producer_owns_its_scope_until_cancel_and_releases_exactly_once() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"produce { acquire: { () => stream (job { ^sh -c 'kill -STOP $$; printf done' }) }, step: { source => do { collect_bytes source; Option.None } }, release: { state reason => print "released-once" } } |> collect"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "released-once")
    );
    terminal.send(b"let handle = (jobs ())[0].handle; bg handle\r")?;
    terminal.until(|s| {
        s.contents().contains("requires fg or cancel") && s.contents().trim_end().ends_with("rill>")
    })?;
    // The failed declaration was not published; retrieve the retained job again.
    terminal.send(b"cancel (jobs ())[0].handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "released-once")
            && s.contents().trim_end().ends_with("rill>")
    })?;
    assert_eq!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .filter(|line| *line == "released-once")
            .count(),
        1
    );
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn pure_evaluation_can_stop_and_cancel_without_publishing_or_running_its_tail() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"let hidden = 42; print "spinning"; let spin = rec { spin () => spin () }; spin (); print "unreachable-tail""#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| s.contents().lines().any(|line| line == "spinning"))?;
    terminal.send(b"\x1a")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"cancel (jobs ())[0].handle; 21 * 2\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    assert!(
        !terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "unreachable-tail")
    );
    terminal.send(b"hidden\r")?;
    terminal.until(|s| s.contents().contains("unknown binding 'hidden'"))?;
    Ok(())
}

#[test]
fn repeated_stop_preserves_the_foreground_callers_remaining_expression() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br"^sh -c 'kill -STOP $$; kill -STOP $$'; 40")?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"let resumed_answer = fg (jobs ())[0].handle + 2; resumed_answer\r")?;
    terminal.prompt_after(row)?;
    assert!(!terminal.parser.screen().contents().contains("JobStopped"));
    terminal.send(b"fg (jobs ())[0].handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    assert_eq!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .filter(|line| *line == "42")
            .count(),
        1
    );
    terminal.send(b"resumed_answer + 1\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "43") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn suspended_stdin_cannot_consume_editor_input_or_grant_another_lease() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(
        br#"print "begin-stdin"; stdin () |> lines |> map { line => do { print "input-consumed"; line } } |> collect"#,
    )?;
    terminal.send(b"\r")?;
    terminal.until(|s| s.contents().lines().any(|line| line == "begin-stdin"))?;
    terminal.send(b"lease-ready\n")?;
    terminal.until(|s| s.contents().lines().any(|line| line == "input-consumed"))?;
    terminal.send(b"\x1a")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"stdin () |> collect_bytes\r")?;
    terminal.until(|s| {
        s.contents().contains("ResourceBusy") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"cancel (jobs ())[0].handle; 6 * 7\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn resumed_stream_callbacks_read_current_environment_and_release_after_completion() -> io::Result<()>
{
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"produce { acquire: { () => stream (job { ^sh -c 'kill -STOP $$; printf done' }) }, step: { source => do { collect_bytes source; print (text (get_env "RILL_RESUMED")); Option.None } }, release: { state reason => print "release-completed" } } |> collect"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"set_env \"RILL_RESUMED\" \"fresh-state\"; fg (jobs ())[0].handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "release-completed")
            && s.contents().trim_end().ends_with("rill>")
    })?;
    assert!(terminal.parser.screen().contents().contains("fresh-state"));
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn foregrounding_an_external_handle_attaches_its_stopped_continuation() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"let worker = start (job { ^sh -c 'kill -STOP $$; kill -STOP $$; printf "child-finished\n"' })"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"fg worker; 41 + 1\r")?;
    terminal.prompt_after(row)?;
    terminal.send(b"fg (jobs ())[1].handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    assert!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "child-finished")
    );
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn stopped_foreground_modes_are_saved_and_restored_separately_from_the_shell() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"^sh -c 'stty -echo; kill -STOP $$; case " $(stty -a) " in *" -echo "*) printf "retained-modes\n";; *) exit 3;; esac'"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"fg (jobs ())[0].handle\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "retained-modes")
            && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"20 + 22\r")?;
    terminal.until(|s| s.contents().lines().any(|line| line == "42"))?;
    Ok(())
}

#[test]
fn force_exit_runs_parked_producer_release_before_terminating() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"produce { acquire: { () => stream (job { ^sh -c 'kill -STOP $$' }) }, step: { source => do { collect_bytes source; Option.None } }, release: { state reason => print "shutdown-release" } } |> collect"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"exit_force 0\r")?;
    terminal.wait_for_exit()?;
    assert!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "shutdown-release")
    );
    Ok(())
}

#[test]
fn blocked_launch_setup_can_stop_and_cancel_without_releasing_targets() -> io::Result<()> {
    for mode in ["run", "capture", "stream", "start", "through"] {
        let mut terminal = Terminal::new()?;
        terminal.until(|s| s.contents().trim_end() == "rill>")?;
        let gate = terminal.home.path().join("stopped-launch-gate");
        let marker = terminal.home.path().join("stopped-launch-marker");
        assert!(Command::new("mkfifo").arg(&gate).status()?.success());
        let plan = if mode == "through" {
            format!(
                "job {{ ^cat | ^cat 2> {:?} > {:?} | ^cat }}",
                marker.to_str().unwrap(),
                gate.to_str().unwrap()
            )
        } else {
            format!(
                "job {{ ^printf never > {:?} < {:?} }}",
                marker.to_str().unwrap(),
                gate.to_str().unwrap()
            )
        };
        let operation = match mode {
            "stream" => format!("stream ({plan}) |> collect_bytes"),
            "through" => {
                format!("chunks (encode_utf8 \"input\") |> through ({plan}) |> collect_bytes")
            }
            mode => format!("{mode} ({plan})"),
        };
        terminal.send(format!("print \"setup-starting\"; {operation}\r").as_bytes())?;
        terminal.until(|s| s.contents().lines().any(|line| line == "setup-starting"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, mode));
            }
            std::thread::yield_now();
        }
        // The helper has created the first redirection but cannot pass the launch barrier.
        let pid =
            rustix::process::Pid::from_raw(i32::try_from(terminal.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::TSTP)?;
        terminal.until(|s| {
            s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
        })?;
        assert!(std::fs::read(&marker)?.is_empty());
        terminal.send(b"cancel (jobs ())[0].handle; 21 * 2\r")?;
        terminal.until(|s| {
            s.contents().lines().any(|line| line == "42")
                && s.contents().trim_end().ends_with("rill>")
        })?;
        assert!(std::fs::read(&marker)?.is_empty());
        terminal.send(b"\x04")?;
        terminal.wait_for_exit()?;
    }
    Ok(())
}

#[test]
fn foreground_resume_continues_the_original_blocked_launch_barrier() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    let gate = terminal.home.path().join("resume-launch-gate");
    let marker = terminal.home.path().join("resume-launch-marker");
    assert!(Command::new("mkfifo").arg(&gate).status()?.success());
    terminal.send(
        format!(
            "print \"launch-pending\"; ^printf resumed > {:?} < {:?}; print \"launch-finished\"\r",
            marker.to_str().unwrap(),
            gate.to_str().unwrap()
        )
        .as_bytes(),
    )?;
    terminal.until(|s| s.contents().lines().any(|line| line == "launch-pending"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::yield_now();
    }
    let pid = rustix::process::Pid::from_raw(i32::try_from(terminal.child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TSTP)?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"fg (jobs ())[0].handle\r")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let writer = loop {
        match open(
            &gate,
            OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(writer) => break writer,
            Err(Errno::NXIO) if Instant::now() < deadline => {
                std::thread::yield_now();
            }
            Err(error) => return Err(error.into()),
        }
    };
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "launch-finished")
            && s.contents().trim_end().ends_with("rill>")
    })?;
    drop(writer);
    assert_eq!(std::fs::read(marker)?, b"resumed");
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn explicit_cancel_reports_release_failure_without_cancelling_its_caller() -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(br#"produce { acquire: { () => stream (job { ^sh -c 'kill -STOP $$' }) }, step: { source => do { collect_bytes source; Option.None } }, release: { state reason => raise (error "ReleaseFailed" "release diagnostic") } } |> collect"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(br#"let caught = attempt { () => cancel (jobs ())[0].handle }; print (match caught of { Result.Err {error} => error.kind, _ => "missed" }); 21 * 2"#)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    assert!(
        terminal
            .parser
            .screen()
            .contents()
            .lines()
            .any(|line| line == "CleanupError")
    );
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn suspended_output_resumes_without_replaying_its_written_prefix() -> io::Result<()> {
    use std::io::Read;
    let (reader, writer) = rustix::pipe::pipe()?;
    let payload = vec![b'x'; 2 * 1024 * 1024];
    let mut terminal = Terminal::configured(|home, command, _| {
        std::fs::write(home.join("payload"), &payload)?;
        command.stdout(Stdio::from(writer));
        Ok(())
    })?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(
        format!(
            "write_bytes (encode_utf8 (read_text {:?}))\r",
            terminal.home.path().join("payload").to_str().unwrap()
        )
        .as_bytes(),
    )?;
    let mut ready = [PollFd::new(&reader, PollFlags::IN)];
    assert_ne!(
        poll(
            &mut ready,
            Some(&Duration::from_secs(10).try_into().unwrap())
        )?,
        0
    );
    let pid = rustix::process::Pid::from_raw(i32::try_from(terminal.child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TSTP)?;
    terminal.until(|s| {
        s.contents().contains("] stopped") && s.contents().trim_end().ends_with("rill>")
    })?;
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::fs::File::from(reader)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let row = terminal.parser.screen().cursor_position().0;
    terminal.send(b"fg (jobs ())[0].handle; print \"done\"\r")?;
    terminal.prompt_after(row)?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()?;
    let mut expected = payload;
    expected.extend_from_slice(b"done\n");
    assert_eq!(output.join().unwrap()?, expected);
    Ok(())
}

#[test]
fn merged_foreground_calls_share_one_terminal_lease() -> io::Result<()> {
    read_two_foreground_calls(br#"do { let first = items [()] |> map { () => run job { ^sh -c "printf 'first-ready\\n'; IFS= read -r reply; test \"$reply\" = answer" } }; let second = items [()] |> map { () => run job { ^sh -c "printf 'second-ready\\n'; IFS= read -r reply; test \"$reply\" = answer" } }; collect (merge [first, second]); 42 }"#)
}

#[test]
fn merged_foreground_handles_share_one_terminal_lease() -> io::Result<()> {
    read_two_foreground_calls(br#"do { let first = start job { ^sh -c "kill -STOP $$; printf 'first-ready\\n'; IFS= read -r reply; test \"$reply\" = answer" < /dev/tty }; let second = start job { ^sh -c "kill -STOP $$; printf 'second-ready\\n'; IFS= read -r reply; test \"$reply\" = answer" < /dev/tty }; attempt { () => wait first }; attempt { () => wait second }; collect (merge [items [first] |> map fg, items [second] |> map fg]); 42 }"#)
}

fn read_two_foreground_calls(source: &[u8]) -> io::Result<()> {
    let mut terminal = Terminal::new()?;
    terminal.until(|s| s.contents().trim_end() == "rill>")?;
    terminal.send(source)?;
    terminal.send(b"\r")?;
    terminal.until(|s| {
        s.contents()
            .lines()
            .any(|line| matches!(line, "first-ready" | "second-ready"))
    })?;
    let first = terminal
        .parser
        .screen()
        .contents()
        .lines()
        .any(|line| line == "first-ready");
    terminal.send(b"answer\r")?;
    terminal.until(|s| {
        s.contents()
            .lines()
            .any(|line| line == if first { "second-ready" } else { "first-ready" })
    })?;
    terminal.send(b"answer\r")?;
    terminal.until(|s| {
        s.contents().lines().any(|line| line == "42") && s.contents().trim_end().ends_with("rill>")
    })?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}

#[test]
fn process_streams_relay_terminal_errors_with_tostop_enabled() -> io::Result<()> {
    let mut terminal = Terminal::configured(|_, _, slave| {
        let mut modes = rustix::termios::tcgetattr(slave)?;
        modes
            .local_modes
            .insert(rustix::termios::LocalModes::TOSTOP);
        rustix::termios::tcsetattr(slave, rustix::termios::OptionalActions::Now, &modes)?;
        Ok(())
    })?;
    terminal.until(|screen| screen.contents().trim_end() == "rill>")?;
    terminal.send(
        b"stream (job { ^sh -c 'printf warning >&2; printf value' }) |> collect; print \"done\"\r",
    )?;
    terminal.until(|screen| {
        screen.contents().contains("warning") && screen.contents().contains("done\nrill>")
    })?;
    terminal.send(b"stream (job { ^true }) |> collect; print \"quiet\"\r")?;
    terminal.until(|screen| screen.contents().contains("quiet\nrill>"))?;
    terminal.send(b"\x04")?;
    terminal.wait_for_exit()
}
