# Rill Shell

A shell for Linux and macOS that connects ordinary commands with structured data,
first-class functions, and reusable job plans.

```rill
stream job { ^printf "%s\n" "3" "1" "2" }
  |> lines
  |> map parse_int
  |> sort_by identity
  |> to_json
  |> write_bytes
```

Output: `[1,2,3]`. The external program produces bytes; `lines` and `parse_int`
turn them into values that ordinary functions can transform.

**Under development.** Commands, the language, structured streams, and job control
are implemented. The current prompt uses basic terminal input; completion,
rich editing, and persistent history are planned. Rill has its own syntax and does not
run POSIX shell scripts. See [current status](docs/status.md) and the
[implementation plan](docs/implementation-plan.md).

## What makes Rill different

- **Commands and values compose explicitly.** `|` connects process bytes; `|>` passes
  values to functions. JSON, text, paths, and raw bytes have explicit conversions.
- **Functions are ordinary values.** Currying, lexical closures, pattern matching,
  immutable records, and proper tail calls support small, reusable programs.
- **Plans can run more than once.** Build a pipeline as data, then run, capture,
  stream, or start it in the background. Inspect completion through structured reports.
- **Arguments stay intact.** No implicit word splitting, wildcard expansion, or
  interpolation inside quotes. Errors report invalid conversions and arithmetic overflow.
- **Streams have clear lifetimes.** Lazy transforms respect demand; early termination
  closes upstream resources. Materialization limits and producer failures are explicit.

## Build and try

Requirements: Linux or macOS, GCC or Clang with GNU C23 and `<stdckdint.h>`, Meson
1.12+, Ninja, and Python 3.14+. Python is needed for development, not to run the shell.

From the repository root:

```sh
meson subprojects download
meson setup build/release --buildtype=release --wrap-mode=nodownload
meson compile -C build/release
meson test -C build/release --print-errorlogs
build/release/src/rillsh
```

Dependency download acquires pinned yyjson source. Subsequent configuration and builds
need no network access. Set `CC=gcc` or `CC=clang` before the initial setup to select a
compiler. The optional [Containerfile](Containerfile) provides a Fedora toolchain.

Run a command or a saved script from your existing shell:

```sh
build/release/src/rillsh -c '^printf "%s\n" "hello"'
build/release/src/rillsh example.rill argument
build/release/src/rillsh --help
```

Script arguments are available as Bytes through `args ()`. Install with
`meson install -C build/release`, using Meson's configured prefix.

## A small tour

External commands start with `^`. A substitution contributes one argument, even when
it contains spaces:

```rill
let message = "hello world"
^printf "%s\n" $message | ^cat
```

Function calls use whitespace. Put configuration first to reuse a partial application:

```rill
fn larger_than limit value = value > limit
let total = [128, 1024, 4096] |> filter (larger_than 1000) |> sum
print (text total + " bytes")
```

This prints `5120 bytes`. Anonymous functions use `{ value => expression }`; the same
pattern syntax works in function parameters, `let`, and `match`.

A `job` describes work without launching it. Each launch gets a new job and a snapshot
of the current environment:

```rill
let greeting = job { ^printf "%s\n" "hello" }
run greeting
let output = capture greeting
write_bytes output.stdout
check output.report
```

Use `execute plan` for foreground I/O with a returned report, or `start plan` followed
by `check (wait handle)` for background work. `fg`, `bg`, and `cancel` control jobs.
Scripts must wait for, foreground, or cancel their background jobs before ending.

At the prompt, incomplete input continues on the next line. Leave `|` or `|>` at the
end of a line when entering a multiline pipeline. Ctrl-Z suspends a foreground
execution; `jobs ()` lists handles and `fg handle` resumes it. Exit with `exit 0` or
Ctrl-D on empty input after finishing or cancelling active work.

Interactive startup reads `rillsh/init.rill` under the XDG configuration directory;
`--no-config` skips it. Color follows terminal capabilities and can be overridden with
`--color=auto|always|never`.

## Explore and contribute

| Start here                                                                 | What it covers                                            |
| -------------------------------------------------------------------------- | --------------------------------------------------------- |
| [Language](docs/language.md)                                               | Functions, data, patterns, errors, and modules            |
| [Execution](docs/execution.md)                                             | Commands, streams, reports, filesystem, and JSON          |
| [Interaction](docs/interaction.md) · [Platform](docs/platform.md)          | Current invocation and target terminal behavior           |
| [Architecture](docs/architecture.md) · [Plan](docs/implementation-plan.md) | Component ownership, design decisions, and remaining work |
| [Development](docs/development.md) · [Testing](docs/testing.md)            | Build profiles, code conventions, and verification        |
| [References](docs/references.md)                                           | Standards and design sources                              |

For a change, read the owning specification, update the relevant tests, and run the
[quality gate](docs/development.md#quality-gate). Bug reports should include a minimal
reproducer, expected and observed behavior, and the platform/compiler used. Avoid
personal paths or sensitive environment data. Generate the internal C API reference
with `meson compile -C build/release api-docs` when Doxygen is installed.
[AGENTS.md](AGENTS.md) gives coding agents a concise working guide.

## License

[MIT](LICENSE). Unicode data retains its [Unicode license](data/unicode/license.txt);
yyjson retains its upstream MIT notice.
