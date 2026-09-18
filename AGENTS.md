# Working on Rill Shell

Rill Shell (`rillsh`) is a GNU C23 shell for Linux and macOS. Read
[status](docs/status.md) before assuming a feature exists. The [implementation
plan](docs/implementation-plan.md) owns scope and dependencies;
[development](docs/development.md) owns code and tool conventions.

## Build and verify

```sh
meson subprojects download
meson setup build/dev --buildtype=debugoptimized --wrap-mode=nodownload
meson compile -C build/dev
meson test -C build/dev --print-errorlogs
```

Select affected suites with `meson test -C build/dev --suite NAME --print-errorlogs`.
Use a separate Clang build with `-Db_sanitize=address,undefined`. Before reporting
completion, run the relevant [full quality gate](docs/development.md#quality-gate),
including formatting, clang-tidy, and Doxygen; keep those tools in PATH. For Python
changes, also run:

```sh
uvx ruff check tools tests/system
uvx ruff format --check tools tests/system
uvx ty check tools tests/system
```

Report actual results and omitted checks. Passing tests do not establish planned
features.

## Change boundaries

- Prefer native C23, libc/POSIX, and Meson facilities. Support GCC and Clang without
  older-C fallbacks or speculative compatibility branches.
- Check allocation sizes, document borrowed lifetimes, and root live C references
  across GC safepoints. Clean up OS resources explicitly, never through GC finalizers.
- Write English Doxygen contracts in boundary headers. Use file introductions for
  orientation and local comments for invariants; avoid repeating the API contract.
  Update the owning specification and link to it instead of duplicating policy.
- Keep yyjson private. Do not edit downloaded dependencies or generated Unicode
  tables; update pinned inputs and regenerate with `python3 tools/unicode.py`.
- Keep personal paths, credentials, machine details, and host-specific container
  tooling out of tracked files. Store local evidence in ignored build directories.
- Preserve unrelated work. Add dependencies, abstractions, or configuration only for
  a project requirement, not to work around a local environment.
