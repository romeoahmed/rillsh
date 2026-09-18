"""Check generator transformations independently of the generated C tables."""

import hashlib
import shutil
import subprocess
import sys
import unittest
from itertools import permutations
from pathlib import Path
from tempfile import TemporaryDirectory

from tools import unicode as generator


class UnicodeGeneratorTests(unittest.TestCase):
    def test_property_records(self) -> None:
        source = """# UCD comments and blank lines carry no properties.

0041 ; Letter # one code point
0300..0302 ; InCB ; Extend
"""
        self.assertEqual(
            list(generator.properties(source)),
            [(0x41, 0x41, ("Letter",)), (0x300, 0x302, ("InCB", "Extend"))],
        )
        self.assertEqual(
            list(generator.selected(source, {"Letter"})), [(0x41, 0x41, "1")]
        )
        self.assertEqual(list(generator.selected(source, {"Absent"})), [])

    def test_ranges_preserve_properties_and_gaps(self) -> None:
        ranges = [(4, 8, "a"), (2, 5, "a"), (2, 3, "a"), (9, 9, "a"), (11, 12, "b")]
        expected = {
            point: prop
            for first, last, prop in ranges
            for point in range(first, last + 1)
        }
        for ordering in permutations(ranges):
            with self.subTest(ordering=ordering):
                normalized = list(generator.coalesced(iter(ordering)))
                actual = {
                    point: prop
                    for first, last, prop in normalized
                    for point in range(first, last + 1)
                }
                self.assertEqual(actual, expected)
                self.assertEqual(normalized, [(2, 9, "a"), (11, 12, "b")])
                self.assertEqual(list(generator.coalesced(normalized)), normalized)

    def test_empty_and_adjacent_properties(self) -> None:
        self.assertEqual(list(generator.coalesced([])), [])
        ranges = [(0, 0, "a"), (1, 1, "b"), (0x10FFFF, 0x10FFFF, "a")]
        self.assertEqual(list(generator.coalesced(reversed(ranges))), ranges)

    def test_invalid_or_conflicting_ranges(self) -> None:
        for ranges in (
            [(-1, 0, "a")],
            [(2, 1, "a")],
            [(0x10FFFF, 0x110000, "a")],
            [(1, 3, "a"), (3, 5, "b")],
            [(1, 9, "a"), (3, 5, "b")],
        ):
            with self.subTest(ranges=ranges), self.assertRaises(ValueError):
                list(generator.coalesced(ranges))

    def test_hash_verification_uses_original_bytes(self) -> None:
        directory = Path(self.enterContext(TemporaryDirectory()))
        path = directory / "property.txt"
        data = b"0041 ; Letter\r\n"
        path.write_bytes(data)
        manifest: generator.Manifest = {
            "version": "test",
            "inputs": [{"file": path.name, "sha256": hashlib.sha256(data).hexdigest()}],
        }
        self.assertEqual(
            generator.verified_inputs(directory, manifest), {path.name: data.decode()}
        )
        path.write_bytes(data.replace(b"\r\n", b"\n"))
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            generator.verified_inputs(directory, manifest)

    def test_cli_regenerates_and_checks_exact_bytes(self) -> None:
        work = Path(self.enterContext(TemporaryDirectory()))
        script = Path(generator.__file__)
        (work / "tools").mkdir()
        shutil.copyfile(script, work / "tools/unicode.py")
        (work / "data").symlink_to(
            script.parent.parent / "data", target_is_directory=True
        )
        (work / "src/text").mkdir(parents=True)
        command = (sys.executable, str(work / "tools/unicode.py"))
        generated = work / "src/text/unicode_tables.inc"

        def invoke(*options: str, status: int = 0) -> None:
            result = subprocess.run(
                (*command, *options),
                check=False,
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, status, result.stderr)

        invoke()
        expected = generated.read_bytes()
        self.assertEqual(
            expected,
            (script.parent.parent / "src/text/unicode_tables.inc").read_bytes(),
        )
        invoke("--check")
        changed = expected.replace(b"\n", b"\r\n")
        generated.write_bytes(changed)
        invoke("--check", status=1)
        self.assertEqual(generated.read_bytes(), changed)
        invoke()
        self.assertEqual(generated.read_bytes(), expected)
