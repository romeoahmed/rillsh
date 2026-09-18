"""Own isolated test directories, environments, and subprocess cleanup."""

import os
import signal
import subprocess
import unittest
from contextlib import ExitStack, suppress
from pathlib import Path
from tempfile import TemporaryDirectory


class ShellCase(unittest.TestCase):
    def setUp(self) -> None:
        self.shell = Path(os.environ["RILL_TEST_SHELL"]).resolve(strict=True)
        self.child = Path(os.environ["RILL_TEST_CHILD"]).resolve(strict=True)
        self.work = Path(self.enterContext(TemporaryDirectory(prefix="rill-test-")))
        (self.work / "child").symlink_to(self.child)
        (self.work / "rillsh").symlink_to(self.shell)
        self.environment = os.environ | {
            "HOME": str(self.work),
            "PATH": os.defpath,
            "LC_ALL": "C",
            "LANG": "C",
            "TZ": "UTC",
            "TERM": "dumb",
            "COLORTERM": "",
            "NO_COLOR": "1",
        }
        for name in ("CONFIG", "CACHE", "DATA", "STATE", "RUNTIME"):
            directory = self.work / name.lower()
            directory.mkdir(mode=0o700)
            self.environment[
                f"XDG_{name}_HOME" if name != "RUNTIME" else "XDG_RUNTIME_DIR"
            ] = str(directory)

    def execute(
        self,
        arguments: tuple[str | Path, ...],
        *,
        status: int = 0,
        environment: dict[str, str] | dict[bytes, bytes] | None = None,
        input: bytes | None = None,
    ) -> subprocess.CompletedProcess[bytes]:
        process = subprocess.Popen(
            arguments,
            cwd=self.work,
            env=self.environment if environment is None else environment,
            stdin=subprocess.DEVNULL if input is None else subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        # Close pipes separately: Popen.__exit__ would wait without a deadline.
        with ExitStack() as pipes:
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream is not None:
                    pipes.enter_context(stream)
            try:
                stdout, stderr = process.communicate(input, timeout=15)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    # Only a still-owned leader authorizes signaling its group.
                    if process.poll() is None:
                        with suppress(ProcessLookupError):
                            os.killpg(process.pid, signal.SIGKILL)
                self.fail(f"command exceeded its deadline: {arguments!r}")
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=3)
        self.assertEqual(
            process.returncode,
            status,
            f"command: {arguments!r}\nstdout: {stdout[-4096:]!r}\nstderr: {stderr[-4096:]!r}",
        )
        return subprocess.CompletedProcess(
            arguments, process.returncode, stdout, stderr
        )
