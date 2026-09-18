#!/usr/bin/env python3
"""Check repository conventions, generated data, C analysis, docs, and tests."""

import argparse
import os
import re
import subprocess
import sys
from collections.abc import Iterable, Iterator
from itertools import groupby
from pathlib import Path

DEPENDENCIES = {
    "text": set(),
    "syntax": {"text"},
    "runtime": {"syntax", "text"},
    "platform": {"text"},
    "exec": {"platform", "text"},
    "library": {"runtime", "exec", "platform", "text"},
    "session": {"syntax", "runtime", "exec", "platform", "text", "library"},
}


def privacy_errors(path: Path, text: str) -> Iterator[str]:
    if re.search(r"/(?:Users|home|private/var|var/folders|opt/homebrew)/", text):
        yield f"{path}: machine-specific path"
    if re.search(r"(?i)\bapple[- ]container\b|com\.apple\.containerization", text):
        yield f"{path}: host-specific container reference"


def repository_files() -> Iterator[Path]:
    result = subprocess.run(
        ("git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"),
        capture_output=True,
        check=True,
    )
    yield from sorted(
        {Path(os.fsdecode(name)) for name in result.stdout.split(b"\0") if name}
    )


def markdown_errors(path: Path, text: str) -> Iterator[str]:
    lines = text.splitlines()
    if any(line.rstrip() != line for line in lines):
        yield f"{path}: trailing whitespace"
    if sum(line.startswith("```") for line in lines) % 2:
        yield f"{path}: unclosed code fence"
    for target in re.findall(r"\]\(([^)]+)\)", text):
        if (
            "://" not in target
            and not target.startswith("#")
            and not (path.parent / target.partition("#")[0]).exists()
        ):
            yield f"{path}: missing link target {target}"
    for is_table, rows in groupby(lines, key=lambda line: line.startswith("|")):
        if is_table and len({len(re.split(r"(?<!\\)\|", row)) for row in rows}) != 1:
            yield f"{path}: inconsistent table columns (escape literal pipes)"


def include_errors(path: Path) -> Iterator[str]:
    owner = path.parts[1]
    if owner in DEPENDENCIES:
        for name in re.findall(
            r'^#include "([^"]+)"', path.read_text(encoding="utf-8"), re.MULTILINE
        ):
            if "/" in name and name.partition("/")[0] not in DEPENDENCIES[owner]:
                yield f"{path}: forbidden component dependency {name}"


def source_files() -> tuple[Path, ...]:
    return tuple(
        sorted(
            path
            for directory in (Path("src"), Path("tests"))
            for path in directory.rglob("*")
            if path.suffix in {".c", ".h"}
        )
    )


def run(command: Iterable[str | Path], root: Path) -> str:
    """Run a command, print checkout-relative output, and stop on failure."""
    result = subprocess.run(
        tuple(map(str, command)),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    output = result.stdout.replace(str(root), ".")
    print(output, end="", flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("build", type=Path, help="Meson build directory")
    args = parser.parse_args()
    build = args.build.absolute()
    root = Path(__file__).resolve().parent.parent
    os.chdir(root)
    build = Path(os.path.relpath(build, root))
    sources = source_files()
    repository = tuple(path for path in repository_files() if path.is_file())
    privacy = tuple(
        error
        for path in repository
        for error in privacy_errors(
            path, path.read_text(encoding="utf-8", errors="replace")
        )
    )
    problems = tuple(
        error
        for path in repository
        if path.suffix == ".md"
        for error in markdown_errors(path, path.read_text(encoding="utf-8"))
    )
    problems += tuple(
        error
        for path in sources
        if path.parts[0] == "src"
        for error in include_errors(path)
    )
    if privacy or problems:
        raise SystemExit("\n".join((*privacy, *problems)))
    run((sys.executable, "tools/unicode.py", "--check"), root)
    run(("clang-tidy", "--verify-config"), root)
    checks = run(("clang-tidy", "--list-checks"), root)
    (build / "clang-tidy-checks.txt").write_text(checks, encoding="utf-8")
    run(
        ("ninja", "-C", build, "clang-format-check", "clang-tidy"),
        root,
    )
    run(("meson", "compile", "-C", build, "api-docs"), root)
    run(("meson", "test", "-C", build, "--print-errorlogs"), root)


if __name__ == "__main__":
    main()
