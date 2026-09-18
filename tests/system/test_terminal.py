"""Verify job control and prompt recovery through a real controlling terminal."""

import errno
import os
import pty
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
    ) -> None:
        self.pending = b""
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            try:
                os.chdir(directory)
                if stdout is not None:
                    with stdout.open("wb") as output:
                        os.dup2(output.fileno(), 1)
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
    def setUp(self) -> None:
        super().setUp()
        self.environment.pop("NO_COLOR")
        self.environment |= {"TERM": "xterm-256color", "COLORTERM": "truecolor"}

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
        terminal.send("wait(j)")
        terminal.expect(b"wait(j)\r\n")
        # A blocked wait must not return a prompt before interruption.
        self.assertFalse(select.select([terminal.fd], [], [], 0.1)[0])
        terminal.write(b"\x03")
        terminal.expect(b"rill> ")
        terminal.send("jobs()[0].state")
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
        terminal.send("jobs()")
        # Each launch collects the acknowledged report from the preceding entry.
        terminal.expect(b"<List 1>")
        terminal.expect(b"rill> ")
        self.assert_restored()
        terminal.finish()

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


if __name__ == "__main__":
    unittest.main(verbosity=2)
