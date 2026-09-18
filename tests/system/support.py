"""Own isolated test directories, environments, and subprocess cleanup."""

import os
import signal
import subprocess
import unittest
from contextlib import suppress
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
        try:
            stdout, stderr = process.communicate(input, timeout=15)
        except subprocess.TimeoutExpired:
            # Give the shell time to cancel/reap its jobs before forced termination.
            process.terminate()
            try:
                process.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                with suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGKILL)
                # Descendants may still hold pipe writers; closing avoids an unbounded drain.
                if process.stdout is not None:
                    process.stdout.close()
                if process.stderr is not None:
                    process.stderr.close()
                process.wait(timeout=3)
            self.fail("command exceeded the process-test deadline")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            if process.stdin is not None:
                process.stdin.close()
            if process.stdout is not None:
                process.stdout.close()
            if process.stderr is not None:
                process.stderr.close()
        self.assertEqual(process.returncode, status, stderr.decode(errors="replace"))
        return subprocess.CompletedProcess(
            arguments, process.returncode, stdout, stderr
        )
