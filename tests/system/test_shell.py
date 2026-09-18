"""Verify source-to-process behavior in isolated shell sessions."""

import errno
import os
import signal
import subprocess
import time

from support import ShellCase


def quote(text: str) -> str:
    escaped = text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    return f'"{escaped}"'


def encoded_arguments(words: tuple[str, ...]) -> bytes:
    return b"".join(
        str(len(data)).encode() + b":" + data + b"\n" for data in map(str.encode, words)
    )


class ShellTests(ShellCase):
    def invoke(
        self,
        source: str,
        status: int = 0,
        *,
        environment: dict[str, str] | None = None,
        closed: tuple[int, ...] = (),
    ) -> subprocess.CompletedProcess[bytes]:
        command = (str(self.shell), "-c", source)
        if closed:
            command = (
                str(self.child),
                "exec-closed",
                "".join(map(str, closed)),
                *command,
            )
        return self.execute(command, status=status, environment=environment)

    def test_invocation(self) -> None:
        for option, marker in (
            ("--help", b"Usage: rillsh"),
            ("--version", b"Rill Shell "),
        ):
            with self.subTest(option=option):
                result = self.execute((self.shell, option))
                self.assertIn(marker, result.stdout)
                self.assertEqual(result.stderr, b"")
        for arguments, reason in (
            (("-c",), b"-c requires exactly one source argument"),
            (("-c", "()", "extra"), b"-c requires exactly one source argument"),
            (("-i", "-c", "()"), b"-i cannot be combined"),
            (("--color=invalid",), b"--color must be auto, always, or never"),
            (("--unknown",), b"unknown option"),
        ):
            with self.subTest(arguments=arguments):
                result = self.execute((self.shell, *arguments), status=2)
                self.assertEqual(result.stdout, b"")
                self.assertIn(reason, result.stderr)
                self.assertIn(b"rillsh --help", result.stderr)
        result = self.invoke("")
        self.assertEqual((result.stdout, result.stderr), (b"", b""))

    def test_argument_bytes(self) -> None:
        words = ("", "a b", "a\nb", "*", "~", "--flag", "\u754c")
        result = self.invoke("^./child args " + " ".join(map(quote, words)))
        self.assertEqual(result.stdout, encoded_arguments(words))
        result = self.invoke("^./child args $(bytes [255, 254])")
        self.assertEqual(result.stdout, b"2:\xff\xfe\n")
        failure = self.invoke('^./child args "\\u{0}"', status=1)
        self.assertIn(b"stage 0, argument 2", failure.stderr)
        self.assertIn(b"TypeError", self.invoke("path (bytes [0])", status=1).stderr)

    def test_launch_and_process_failures_are_distinct(self) -> None:
        self.invoke("^./child exit 127", status=127)
        self.assertIn(b"LaunchError", self.invoke("^./missing", status=1).stderr)
        script = self.work / "plain"
        script.write_text("echo SHOULD_NOT_RUN\n", encoding="utf-8")
        script.chmod(0o755)
        self.assertIn(b"LaunchError", self.invoke("^./plain", status=1).stderr)

    def test_redirection_order(self) -> None:
        output = self.work / "output"
        self.invoke("^./child both > output 2>&1")
        self.assertEqual(output.read_bytes(), b"out\nerr\n")
        self.assertEqual(self.invoke("^./child both 2>&1 > output").stdout, b"err\n")
        self.assertEqual(output.read_bytes(), b"out\n")
        self.invoke("^./child both >> output")
        self.assertEqual(output.read_bytes(), b"out\nout\n")
        errors = self.work / "errors"
        self.assertEqual(self.invoke("^./child both 2> errors").stdout, b"out\n")
        self.assertEqual(errors.read_bytes(), b"err\n")
        self.assertEqual(self.invoke("^./child both 2>> errors").stdout, b"out\n")
        self.assertEqual(errors.read_bytes(), b"err\nerr\n")
        self.assertEqual(
            self.invoke("^./child both > output | ^./child echo").stdout, b""
        )
        self.assertEqual(output.read_bytes(), b"out\n")
        for operator in ("3>", "2>&3", "1>", "<<"):
            with self.subTest(operator=operator):
                self.invoke(f"^./child args {operator} output", status=2)

    def test_large_pipeline_and_partial_launch(self) -> None:
        payload = bytes(range(256)) * 8192
        (self.work / "input").write_bytes(payload)
        pipeline = "^./child echo < input | ^./child echo"
        self.assertEqual(self.invoke(pipeline + " | ^./child echo").stdout, payload)
        self.invoke(pipeline + " > output")
        self.assertEqual((self.work / "output").read_bytes(), payload)
        self.invoke(pipeline + " | ^./missing", status=1)

    def test_parse_before_effects(self) -> None:
        self.invoke("^./child mark forbidden; ^cat |", status=2)
        self.assertFalse((self.work / "forbidden").exists())

    def test_plan_reuse_and_background_acknowledgement(self) -> None:
        result = self.invoke("let p = job { ^./child args once }; run p; run p")
        self.assertEqual(result.stdout, b"4:once\n" * 2)
        self.invoke("let j = start job { ^./child exit 0 }; wait j")
        self.invoke("start job { ^./child exit 0 }", status=1)
        self.invoke("let j = start job { ^./child exit 7 }; check (wait j)", status=7)

    def test_path_lookup(self) -> None:
        for path in ("", "missing:."):
            with self.subTest(path=path):
                result = self.invoke(
                    "^child args cwd", environment=self.environment | {"PATH": path}
                )
                self.assertEqual(result.stdout, b"3:cwd\n")
        environment = {
            name: value for name, value in self.environment.items() if name != "PATH"
        }
        self.invoke("^true", environment=environment)
        denied = self.work / "denied"
        denied.write_text("x", encoding="utf-8")
        denied.chmod(0o644)
        result = self.invoke(
            "^denied", status=1, environment=self.environment | {"PATH": "."}
        )
        self.assertIn(b"LaunchError", result.stderr)

    def test_directory_and_locale(self) -> None:
        destination = self.work / "directory"
        destination.mkdir()
        result = self.invoke('cd "directory"; ^../child cwd')
        self.assertEqual(result.stdout, os.fsencode(destination.resolve()))
        environment = self.environment | {"LC_ALL": "C", "LANG": "preserved"}
        self.assertEqual(
            self.invoke("^./child env LANG", environment=environment).stdout,
            b"preserved",
        )

    def test_explicit_script(self) -> None:
        (self.work / "-script").write_text(
            "#!/usr/bin/env rillsh\n^./child args script\n", encoding="utf-8"
        )
        result = self.execute((self.shell, "--", "-script"))
        self.assertEqual(result.stdout, b"6:script\n")

    def test_scripts_do_not_load_interactive_configuration(self) -> None:
        directory = self.work / "config" / "rillsh"
        directory.mkdir()
        (directory / "init.rill").write_text(
            "^./child mark forbidden\n", encoding="utf-8"
        )
        self.invoke("()")
        self.execute((self.shell,), input=b"()")
        (self.work / "script").write_text("()", encoding="utf-8")
        self.execute((self.shell, "script"))
        self.assertFalse((self.work / "forbidden").exists())

    def test_executable_shebang(self) -> None:
        script = self.work / "script"
        script.write_text(
            "#!/usr/bin/env rillsh\n^./child args shebang\n", encoding="utf-8"
        )
        script.chmod(0o700)
        result = self.execute(
            ("./script",), environment=self.environment | {"PATH": "."}
        )
        self.assertEqual(result.stdout, b"7:shebang\n")

    def test_environment_and_path_bytes(self) -> None:
        environment = {
            os.fsencode(key): os.fsencode(value)
            for key, value in self.environment.items()
        }
        environment[b"RILL_BYTES"] = b"\xff\xfe"
        result = self.execute(
            (self.shell, "-c", "^./child env RILL_BYTES"), environment=environment
        )
        self.assertEqual(result.stdout, b"\xff\xfe")
        result = self.invoke("^./child args $(path (bytes [255, 254]))")
        self.assertEqual(result.stdout, b"2:\xff\xfe\n")

    def test_closed_standard_descriptors(self) -> None:
        for descriptor in (0, 1, 2):
            with self.subTest(descriptor=descriptor):
                self.invoke("^./child signals", closed=(descriptor,))
        self.invoke("^./child both > output 2>&1", closed=(0, 1, 2))
        self.assertEqual((self.work / "output").read_bytes(), b"out\nerr\n")

    def test_diagnostic_filename_is_escaped(self) -> None:
        name = "script\x1b[31m\n"
        (self.work / name).write_text('"\\q', encoding="utf-8")
        result = self.execute((self.shell, "--color=never", "--", name), status=2)
        self.assertNotIn(b"\x1b", result.stderr)
        self.assertIn(b"script\\x1b[31m\\x0a", result.stderr)
        self.assertEqual(result.stderr.count(b"\n"), 1)

    def test_redirected_source_and_invalid_utf8(self) -> None:
        result = self.execute((self.shell,), input=b"^./child args stdin\n")
        self.assertEqual(result.stdout, b"5:stdin\n")
        for source in (b"\xff", b"^./child mark forbidden;\0"):
            with self.subTest(source=source):
                self.execute((self.shell,), input=source, status=2)
                self.assertFalse((self.work / "forbidden").exists())

    def test_background_stdin_is_disconnected(self) -> None:
        self.invoke("let j = start job { ^./child echo }; check (wait j)")

    def test_pipeline_checks_each_stage(self) -> None:
        self.invoke("^./child exit 7 | ^./child exit 0", status=7)
        self.invoke("^./child exit 0 | ^./child exit 9", status=9)
        self.invoke("^./child produce | ^./child take")

    def test_termination_while_waiting_for_source(self) -> None:
        for event, status in ((signal.SIGTERM, 143), (signal.SIGINT, 130)):
            with self.subTest(signal=event):
                self.assert_source_interrupted(event, status)

    def assert_source_interrupted(self, event: signal.Signals, status: int) -> None:
        source = self.work / "source"
        source.unlink(missing_ok=True)
        os.mkfifo(source)
        process = subprocess.Popen(
            (self.shell, "source"), cwd=self.work, env=self.environment
        )
        try:
            deadline = time.monotonic() + 5
            while True:
                try:
                    writer = os.open(source, os.O_WRONLY | os.O_NONBLOCK)
                    break
                except OSError as error:
                    if error.errno != errno.ENXIO:
                        raise
                    self.assertIsNone(
                        process.poll(), "shell exited before opening source"
                    )
                    self.assertLess(time.monotonic(), deadline, "source-open deadline")
                    time.sleep(0.01)
            # Connecting the FIFO proves initialization has installed signal handlers.
            with os.fdopen(writer, "wb"):
                process.send_signal(event)
                self.assertEqual(process.wait(timeout=5), status)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=3)
