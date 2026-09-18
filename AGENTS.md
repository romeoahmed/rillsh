# Working on Rill Shell

Rill Shell (`rillsh`) is a GNU C23 shell for Linux and macOS. Read
[status](docs/status.md) for implemented features, the
[plan](docs/implementation-plan.md) for scope, and
[development](docs/development.md) for code and tool conventions.

## Build and verify

```sh
meson subprojects download
meson setup build/dev --buildtype=debugoptimized --wrap-mode=nodownload
meson compile -C build/dev
meson test -C build/dev --print-errorlogs
```

Select affected suites with `--suite NAME`. Use a separate Clang build with
`-Db_sanitize=address,undefined`. Before completion, run the applicable
[quality gate](docs/development.md#quality-gate), including formatting, clang-tidy,
and Doxygen; keep these tools in PATH. For Python changes, also run:

```sh
uvx ruff check tools tests
uvx ruff format --check tools tests
uvx ty check tools tests
```

Report results and omitted checks. Tests do not establish unimplemented features.

## Change boundaries

- Prefer native C23, libc/POSIX, and Meson facilities. Support GCC and Clang without
  older-C fallbacks or speculative compatibility branches.
- Check allocation sizes, document borrowed lifetimes, and root live C references
  across GC safepoints. Release OS resources explicitly, never through GC finalizers.
- Put English Doxygen contracts in boundary headers and implementation invariants
  near the code. Keep behavior in its owning specification; link rather than repeat.
- Keep yyjson private. Do not edit downloaded dependencies or generated Unicode
  tables; update pinned inputs and regenerate with `python3 tools/unicode.py`.
- Keep personal paths, credentials, machine details, and host-specific container
  commands out of tracked files. Put local evidence in ignored build directories.
- Preserve unrelated work. Add dependencies, abstractions, or configuration only for
  a project requirement, not a local environment workaround.
