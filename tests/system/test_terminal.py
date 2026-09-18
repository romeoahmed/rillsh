"""Verify job control and prompt recovery through a real controlling terminal."""

import errno
import os
import pty
import resource
import select
import signal
import termios
import time
import unittest
from contextlib import AbstractContextManager, suppress
from pathlib import Path

from support import ShellCase


class Terminal(AbstractContextManager["Terminal"]):
    """Own the PTY and direct child; cleanup also terminates the foreground job."""

    def __init__(
        self,
        shell: Path,
        directory: Path,
        environment: dict[str, str],
        *,
        stdout: Path | None = None,
        arguments: tuple[str, ...] = ("--no-config",),
        reserve_fds: int = 0,
        blocked_signals: tuple[signal.Signals, ...] = (),
    ) -> None:
        self.pending = b""
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                if blocked_signals:
                    signal.pthread_sigmask(signal.SIG_BLOCK, blocked_signals)
                os.chdir(directory)
                if stdout is not None:
                    with stdout.open("wb") as output:
                        os.dup2(output.fileno(), 1)
                if reserve_fds:
                    # These descriptors deliberately survive exec; child exit owns cleanup.
                    source = os.open(os.devnull, os.O_RDONLY)
                    os.set_inheritable(source, True)
                    for fd in range(3, reserve_fds):
                        if fd != source:
                            os.dup2(source, fd, inheritable=True)
                os.execve(shell, ("rillsh", *arguments), environment)
            finally:
                os._exit(127)
        os.set_blocking(self.fd, False)

    def write(self, data: bytes) -> None:
        deadline = time.monotonic() + 8
        remaining = memoryview(data)
        while remaining:
            timeout = deadline - time.monotonic()
            if timeout <= 0 or not select.select([], [self.fd], [], timeout)[1]:
                raise AssertionError("terminal write deadline expired")
            remaining = remaining[os.write(self.fd, remaining) :]

    def send(self, text: str) -> None:
        self.write(text.encode() + b"\n")

    def read(self, deadline: float) -> bytes:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([self.fd], [], [], remaining)[0]:
            raise AssertionError("terminal event deadline expired")
        try:
            return os.read(self.fd, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                return b""
            raise

    def expect(self, marker: bytes, timeout: float = 8) -> bytes:
        deadline = time.monotonic() + timeout
        while marker not in self.pending:
            try:
                chunk = self.read(deadline)
            except AssertionError as error:
                raise AssertionError(
                    f"waiting for {marker!r}; terminal tail: {self.pending[-4096:]!r}"
                ) from error
            if not chunk:
                raise AssertionError(
                    f"terminal closed before {marker!r}: {self.pending[-4096:]!r}"
                )
            self.pending += chunk
            if len(self.pending) > 1024 * 1024:
                raise AssertionError("terminal output exceeded the fixture limit")
        before, self.pending = self.pending.split(marker, 1)
        return before

    def prompt_after(self, marker: bytes) -> None:
        # Background output and the prompt can legally arrive in either order.
        if b"rill> " not in self.expect(marker):
            self.expect(b"rill> ")

    def wait_for_shell_foreground(self) -> None:
        deadline = time.monotonic() + 8
        while os.tcgetpgrp(self.fd) != self.pid:
            if time.monotonic() >= deadline:
                raise AssertionError("shell did not reclaim the terminal")
            time.sleep(0.01)

    def reap(self, deadline: float) -> int:
        while True:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.pid = 0
                return os.waitstatus_to_exitcode(status)
            if time.monotonic() >= deadline:
                raise AssertionError("terminal child exit deadline expired")
            time.sleep(0.01)

    def wait_for_exit(self, code: int = 0) -> None:
        deadline = time.monotonic() + 8
        while self.read(deadline):
            pass
        # Drain before reaping: Darwin may defer slave teardown until output is consumed.
        actual = self.reap(deadline)
        if actual != code:
            raise AssertionError(
                f"interactive exit status: expected {code}, got {actual}"
            )

    def finish(self, code: int = 0) -> None:
        self.send(f"exit({code})")
        self.wait_for_exit(code)

    def __exit__(self, *_: object) -> None:
        try:
            if self.pid:
                with suppress(OSError, AssertionError):
                    self.send("exit_force(0)")
                    self.wait_for_exit()
                if self.pid:
                    with suppress(OSError):
                        group = os.tcgetpgrp(self.fd)
                        if group > 0 and group != os.getpgrp():
                            os.killpg(group, signal.SIGKILL)
                    with suppress(ProcessLookupError):
                        os.kill(self.pid, signal.SIGKILL)
                    self.reap(time.monotonic() + 3)
        finally:
            os.close(self.fd)


class TerminalTests(ShellCase):
    def test_redirection_does_not_acquire_terminal(self) -> None:
        master, slave = pty.openpty()
        try:
            path = os.ttyname(slave)
            self.execute((self.shell, "-c", f"^./child no-terminal > '{path}'"))
        finally:
            os.close(slave)
            os.close(master)

    def test_inherited_signal_mask(self) -> None:
        terminal = self.enterContext(
            Terminal(
                self.shell,
                self.work,
                self.environment,
                blocked_signals=(signal.SIGINT, signal.SIGTERM, signal.SIGCHLD),
            )
        )
        terminal.expect(b"rill> ")
        terminal.send("fn loop()=>loop(); ^./child args ready; loop()")
        terminal.expect(b"\r\n5:ready\r\n")
        terminal.wait_for_shell_foreground()
        terminal.write(b"\x03")
        terminal.expect(b"rill> ")
        os.kill(terminal.pid, signal.SIGTERM)
        terminal.wait_for_exit(143)

    def setUp(self) -> None:
        super().setUp()
        self.environment.pop("NO_COLOR")
        self.environment |= {"TERM": "xterm-256color", "COLORTERM": "truecolor"}

    def test_functional_entries_and_failed_publication(self) -> None:
        terminal = self.open_terminal()
        terminal.send("let x=4; fn saved(y)=>x+y")
        terminal.expect(b"rill> ")
        terminal.send("let x=90; saved(2)")
        self.assertIn(b"6\r\n", terminal.expect(b"rill> "))
        terminal.send('let x=8; set_env("RILL_EFFECT","kept"); missing')
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        terminal.send(
            'exit(if x==90 and get_env("RILL_EFFECT")==some(bytes([107,101,112,116])) then 0 else 1)'
        )
        terminal.wait_for_exit()

    def test_pure_evaluation_stop_and_publication(self) -> None:
        terminal = self.open_terminal()
        terminal.send(
            'let answer=do {let captured=42; fn loop()=>if get_env("GO")==some(encode_utf8("yes")) then captured else loop(); ^./child args ready; loop()}'
        )
        terminal.expect(b"5:ready\r\n")
        terminal.wait_for_shell_foreground()
        terminal.write(b"\x1a")
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send(
            'let saved=jobs()[0].handle; let intervening=9; set_env("GO","yes")'
        )
        terminal.expect(b"rill> ")
        terminal.send("bg(saved)")
        self.assertIn(b"requires foreground", terminal.expect(b"rill> "))
        terminal.send("fg(saved)")
        terminal.expect(b"rill> ")
        terminal.send("exit(if answer==42 and intervening==9 then 0 else 1)")
        terminal.wait_for_exit()

    def test_mixed_evaluation_resume_and_display(self) -> None:
        terminal = self.open_terminal()
        terminal.send(
            'let result=stream(job { ^./child selfstop }) |> lines |> map(fn(x)=>x+"!") |> collect'
        )
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send("let saved=jobs()[0].handle; wait(saved)")
        self.assertIn(b"requires foreground", terminal.expect(b"rill> "))
        terminal.send("fg(jobs()[0].handle)")
        terminal.expect(b"rill> ")
        terminal.send('if result==["selfstop!","continued!"] then 42 else 0')
        self.assertIn(b"42\r\n", terminal.expect(b"rill> "))
        terminal.send('chunks(encode_utf8("first\\nsecond\\n")) |> lines')
        rendered = terminal.expect(b"rill> ")
        self.assertIn(b"first\r\nsecond\r\n", rendered)
        self.assert_restored()
        terminal.finish()

    def test_stream_interrupt_and_tostop_relay(self) -> None:
        terminal = self.open_terminal()
        modes = termios.tcgetattr(terminal.fd)
        modes[3] |= termios.TOSTOP
        termios.tcsetattr(terminal.fd, termios.TCSANOW, modes)
        terminal.send(
            'collect_bytes(stream(job { ^./child both }))==encode_utf8("out\\n")'
        )
        output = terminal.expect(b"rill> ")
        self.assertIn(b"err\r\n", output)
        self.assertIn(b"true\r\n", output)
        terminal.send(
            "let unpublished=attempt(fn()=>stream(job { ^./child rows }) |> lines |> each(fn(x)=>()))"
        )
        terminal.expect(b"stream-ready\r\n")
        terminal.write(b"\x03")
        terminal.expect(b"rill> ")
        terminal.send("unpublished")
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        terminal.finish()

    def test_suspended_errors_conflicts_and_cancellation(self) -> None:
        terminal = self.open_terminal()
        terminal.send('do { ^./child selfstop; raise(error("Example","retained")) }')
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send(
            'match attempt(fn()=>fg(jobs()[0].handle)) {Result.Err {error}=>error.kind=="Example" and error.message=="retained", _=>false}'
        )
        self.assertIn(b"true\r\n", terminal.expect(b"rill> "))
        terminal.send("struct Shared {value}; ^./child selfstop")
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send("struct Shared {other}")
        terminal.expect(b"rill> ")
        terminal.send("fg(jobs()[0].handle)")
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        terminal.send("let abandoned=do { ^./child selfstop; 42 }")
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send("exit(0)")
        self.assertIn(b"ProcessError", terminal.expect(b"rill> "))
        terminal.send("cancel(jobs()[0].handle); abandoned")
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        terminal.finish()

    def test_multiple_suspended_contexts_and_force_exit(self) -> None:
        terminal = self.open_terminal()
        for name in ("first", "second"):
            terminal.send(f"let {name}=do {{ ^./child selfstop; 42 }}")
            terminal.expect(b"evaluation stopped")
            terminal.expect(b"rill> ")
        terminal.send(
            'let saved=map(fn(j)=>j.handle,filter(fn(j)=>j.kind=="evaluation",jobs())); length(saved)'
        )
        self.assertIn(b"2\r\n", terminal.expect(b"rill> "))
        terminal.send("fg(saved[1]); fg(saved[0])")
        terminal.expect(b"rill> ")
        terminal.send("first+second")
        self.assertIn(b"84\r\n", terminal.expect(b"rill> "))
        terminal.send("stream(job { ^./child selfstop }) |> lines |> collect")
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send("exit_force(0)")
        terminal.wait_for_exit()

    def test_stopped_files_keep_directory_identity(self) -> None:
        source = self.work / "source"
        source.mkdir()
        (source / "data").write_bytes(b"abc")
        (self.work / "other").mkdir()
        terminal = self.open_terminal()
        terminal.send(
            'let sizes=files(path("source")) |> map(fn(e)=>do {^./child selfstop; e.size}) |> collect'
        )
        terminal.expect(b"evaluation stopped")
        terminal.expect(b"rill> ")
        terminal.send('cd(path("other"))')
        terminal.expect(b"rill> ")
        terminal.send("fg(jobs()[0].handle)")
        terminal.expect(b"rill> ")
        terminal.send("exit(if sizes==[3] then 0 else 1)")
        terminal.wait_for_exit()

    def test_startup_configuration_and_opt_out(self) -> None:
        directory = self.work / "config" / "rillsh"
        directory.mkdir()
        (directory / "init.rill").write_text("let configured=42\n", encoding="utf-8")
        terminal = self.enterContext(
            Terminal(self.shell, self.work, self.environment, arguments=())
        )
        terminal.expect(b"rill> ")
        terminal.send("exit(configured-42)")
        terminal.wait_for_exit()
        terminal = self.open_terminal()
        terminal.send("configured")
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        terminal.finish()

    def test_failed_module_can_be_retried(self) -> None:
        module = self.work / "retry.rill"
        module.write_text("export let value=missing\n", encoding="utf-8")
        terminal = self.open_terminal()
        terminal.send('import "./retry.rill" as retry')
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        module.write_text("export let value=7\n", encoding="utf-8")
        terminal.send('import "./retry.rill" as retry; exit(retry.value-7)')
        terminal.wait_for_exit()

    def test_configuration_path_policy(self) -> None:
        default = self.work / ".config" / "rillsh"
        explicit = self.work / "config" / "rillsh"
        for directory, value in ((default, 7), (explicit, 42)):
            directory.mkdir(parents=True)
            (directory / "init.rill").write_text(
                f"let configured={value}\n", encoding="utf-8"
            )
        for xdg, value in (
            (None, 7),
            ("", 7),
            ("config", 7),
            (str(explicit.parent), 42),
        ):
            with self.subTest(xdg=xdg):
                environment = self.environment.copy()
                if xdg is None:
                    environment.pop("XDG_CONFIG_HOME")
                else:
                    environment["XDG_CONFIG_HOME"] = xdg
                with Terminal(
                    self.shell, self.work, environment, arguments=()
                ) as terminal:
                    terminal.expect(b"rill> ")
                    terminal.send(f"exit(configured-{value})")
                    terminal.wait_for_exit()
        environment = self.environment | {"HOME": "", "XDG_CONFIG_HOME": "config"}
        with Terminal(self.shell, self.work, environment, arguments=()) as terminal:
            self.assertIn(b"configuration disabled", terminal.expect(b"rill> "))
            terminal.send("configured")
            self.assertIn(b"TypeError", terminal.expect(b"rill> "))
            terminal.finish()

    def test_attempt_cannot_catch_interrupt(self) -> None:
        terminal = self.open_terminal()
        terminal.send(
            "fn loop(n)=>loop(n+1); attempt(fn()=>do { ^./child args ready; loop(0) })"
        )
        terminal.expect(b"\r\n5:ready\r\n")
        terminal.wait_for_shell_foreground()
        terminal.write(b"\x03")
        terminal.expect(b"rill> ")
        terminal.send("loop")
        self.assertIn(b"TypeError", terminal.expect(b"rill> "))
        terminal.finish()

    def open_terminal(self) -> Terminal:
        self.terminal = self.enterContext(
            Terminal(self.shell, self.work, self.environment)
        )
        self.assertIn(b"38;2;", self.terminal.expect(b"rill> "))
        self.modes = termios.tcgetattr(self.terminal.fd)
        return self.terminal

    def assert_restored(self) -> None:
        self.assertEqual(termios.tcgetattr(self.terminal.fd), self.modes)

    def test_prompt_with_redirected_stdout(self) -> None:
        output = self.work / "stdout"
        with Terminal(
            self.shell, self.work, self.environment, stdout=output
        ) as terminal:
            self.assertIn(b"38;2;", terminal.expect(b"rill> "))
            terminal.send("^./child args redirected")
            terminal.expect(b"rill> ")
            terminal.finish()
        self.assertEqual(output.read_bytes(), b"10:redirected\n")

    def test_noninteractive_does_not_seize_terminal(self) -> None:
        with Terminal(
            self.shell,
            self.work,
            self.environment,
            arguments=("-c", "^./child terminal-owner"),
        ) as terminal:
            output = terminal.expect(b"noninteractive\r\n")
            self.assertNotIn(b"rill> ", output)
            terminal.wait_for_exit()

    def test_foreground_stop_resume_and_input(self) -> None:
        terminal = self.open_terminal()
        terminal.send("^./child stop")
        terminal.expect(b"stopping\r\n")
        terminal.expect(b"rill> ")
        self.assertTrue(termios.tcgetattr(terminal.fd)[3] & termios.ECHO)
        terminal.send("fg(jobs()[0].handle)")
        terminal.expect(b"resumed\r\n")
        terminal.expect(b"rill> ")
        self.assert_restored()
        terminal.send("^./child readtty")
        terminal.expect(b"reading\r\n")
        terminal.send("hello")
        terminal.expect(b"received:hello\r\n")
        terminal.expect(b"rill> ")
        terminal.finish()

    def test_cancellation_and_escalation(self) -> None:
        terminal = self.open_terminal()
        terminal.send("let j = start(job { ^./child ignore-term })")
        terminal.prompt_after(b"ready\r\n")
        terminal.send("cancel(j)")
        terminal.expect(b"rill> ", timeout=5)
        terminal.send("^./child ignore-term")
        terminal.expect(b"ready\r\n")
        terminal.write(b"\x03")
        terminal.expect(b"rill> ", timeout=5)
        self.assert_restored()
        terminal.finish()

    def test_background_stop_resume_and_launch_error(self) -> None:
        terminal = self.open_terminal()
        terminal.send("let stopped = start(job { ^./child selfstop })")
        terminal.prompt_after(b"selfstop\r\n")
        terminal.send("wait(stopped)")
        terminal.expect(b"job stopped")
        terminal.expect(b"rill> ")
        terminal.send("bg(stopped)")
        terminal.prompt_after(b"continued\r\n")
        terminal.send("wait(stopped)")
        terminal.expect(b"rill> ")
        terminal.send("^./missing")
        terminal.expect(b"LaunchError")
        terminal.expect(b"rill> ")
        self.assert_restored()
        terminal.finish()

    def test_interrupt_wait_preserves_background_job(self) -> None:
        terminal = self.open_terminal()
        terminal.send("let j = start(job { ^./child ignore-term })")
        terminal.prompt_after(b"ready\r\n")
        terminal.send("^./child args waiting; wait(j)")
        terminal.expect(b"\r\n7:waiting\r\n")
        terminal.wait_for_shell_foreground()
        # No prompt should have been consumed while waiting for the live job.
        self.assertNotIn(b"rill> ", terminal.pending)
        terminal.write(b"\x03")
        terminal.expect(b"rill> ")
        terminal.send("length(filter(fn(item)=>item.state=='Running',jobs()))")
        terminal.expect(b"\r\n1\r\n")
        terminal.expect(b"rill> ")
        terminal.send("cancel(j)")
        terminal.expect(b"rill> ")
        self.assert_restored()
        terminal.finish()

    def test_invalid_input_and_failed_launches_do_not_accumulate(self) -> None:
        terminal = self.open_terminal()
        terminal.send('"\\q')
        terminal.expect(b"SyntaxError")
        terminal.expect(b"rill> ")
        for _ in range(4):
            terminal.send("^./missing")
            terminal.expect(b"LaunchError")
            terminal.expect(b"rill> ")
        self.assert_restored()
        terminal.send("exit(if length(jobs())<4 then 0 else 1)")
        terminal.wait_for_exit()

    def test_multiline_entry_and_interrupt_at_prompt(self) -> None:
        terminal = self.open_terminal()
        terminal.send('^./child args "first')
        terminal.expect(b"... ")
        terminal.send('second"')
        terminal.expect(b"12:first\r\nsecond\r\n")
        terminal.expect(b"rill> ")
        terminal.write(b"\x03")
        terminal.expect(b"rill> ")
        terminal.send("^./child args recovered")
        terminal.expect(b"9:recovered\r\n")
        terminal.expect(b"rill> ")
        self.assert_restored()
        terminal.finish()

    def test_eof_exits_cleanly(self) -> None:
        terminal = self.open_terminal()
        terminal.write(b"\x04")
        terminal.wait_for_exit()

    def test_high_descriptor_terminal(self) -> None:
        # Cross the historical select bitmap boundary without raising OS limits.
        minimum = 1100
        soft, _ = resource.getrlimit(resource.RLIMIT_NOFILE)
        if soft != resource.RLIM_INFINITY and soft < minimum + 64:
            self.skipTest("descriptor limit too low for the high-FD terminal case")
        with Terminal(
            self.shell, self.work, self.environment, reserve_fds=minimum
        ) as terminal:
            terminal.expect(b"rill> ")
            terminal.send("^./child args high-fd")
            terminal.expect(b"7:high-fd\r\n")
            terminal.expect(b"rill> ")
            terminal.write(b"\x04")
            terminal.wait_for_exit()


if __name__ == "__main__":
    unittest.main(verbosity=2)
