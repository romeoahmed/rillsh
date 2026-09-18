# Working on Rill Shell

Rill Shell (`rillsh`) is a GNU C23 shell for Linux and macOS. Read
[docs/status.md](docs/status.md) before assuming a feature exists. The four-stage scope
and dependency boundaries are in [docs/implementation-plan.md](docs/implementation-plan.md);
[docs/development.md](docs/development.md) owns implementation and tool conventions.

## Build and verify

```sh
meson subprojects download
meson setup build/dev --buildtype=debugoptimized --wrap-mode=nodownload
meson compile -C build/dev
meson test -C build/dev --print-errorlogs
python3 tools/check.py build/dev
```

The last command requires clang-format, clang-tidy, and Doxygen in PATH. For sanitizer
work, use a separate Clang build with `-Db_sanitize=address,undefined`. Run affected
Meson suites during development and the relevant full gate before reporting completion.
For Python changes, run `uvx ruff check tools tests/system`,
`uvx ruff format --check tools tests/system`, and `uvx ty check tools tests/system`.
Report actual results and any checks not run.

## Change conventions

- Prefer native C23, libc/POSIX, and Meson facilities. Support GCC and Clang; add no
  older-C fallbacks or speculative platform branches.
- Keep allocation sizes checked, borrowed lifetimes explicit, and live C references
  rooted across GC safepoints. OS resources have explicit cleanup, never GC finalizers.
- Put concise Doxygen contracts in boundary headers and reasoning beside implementation.
  Keep comments and documentation in English; synchronize the owning specification.
- Keep yyjson private. Do not edit downloaded dependencies or generated Unicode tables;
  update pinned inputs and regenerate with `python3 tools/unicode.py` when required.
- Keep personal paths, credentials, machine details, and host-specific container tooling
  out of tracked files. Build output belongs in ignored build directories.
- Preserve unrelated work. Do not add dependencies, abstractions, or compatibility
  layers merely to simplify a local edit.
