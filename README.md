# Rill Shell

A shell for Linux and macOS, written in GNU C23. Rill Shell combines explicit Unix
process pipelines with a small, dynamically typed functional language.

The executable is `rillsh`. The command foundation is implemented; the complete
language and expression editor are still in development. This is an experimental
shell with its own syntax, not a replacement for running existing shell scripts.

## A first look

Run an external command with `^`, and connect processes with `|`:

```rill
let greeting = "hello world"
^printf "%s\n" $greeting | ^cat
```

Arguments stay intact: `$greeting` supplies one argument, with no word splitting or
implicit wildcard expansion. Quoted strings do not interpolate.

Describe work once and run it more than once:

```rill
let greeting = job { ^printf "%s\n" "hello" }
run(greeting)
run(greeting)
```

A plan stores evaluated arguments. It starts no process and opens no redirection file
until launched. Each launch creates a separate job with its own environment snapshot.

Background jobs have explicit handles:

```rill
let task = start(job { ^sleep 1 })
check(wait(task))
```

`wait` returns a report; `check` turns an unsuccessful report into an error. Use
`fg`, `bg`, and `cancel` to control jobs. Scripts must acknowledge background jobs
before reaching EOF.

## Build and try it

Requirements: Linux or macOS, a GCC or Clang toolchain with GNU C23 and
`<stdckdint.h>`, Meson 1.12 or newer, Ninja, and Python 3.14 or newer. Meson downloads
the pinned yyjson source in the first command; subsequent configuration is offline.
Python is needed for development and tests, not to run the installed shell.

```sh
meson subprojects download
meson setup build/release --buildtype=release --wrap-mode=nodownload
meson compile -C build/release
meson test -C build/release --print-errorlogs
build/release/src/rillsh
```

Select a compiler with `CC=gcc` or `CC=clang` when setting up a new build directory.
A portable Fedora development image is provided in [Containerfile](Containerfile);
using a container is optional.

From your existing shell, run one command or save the first example as `example.rill`
and execute it:

```sh
build/release/src/rillsh -c '^printf "%s\n" "hello"'
build/release/src/rillsh example.rill
build/release/src/rillsh --help
```

Running without arguments on a terminal opens the prompt. It accepts multiline input
when the parser needs more source. Enter `exit(0)` or press Ctrl-D on an empty prompt
to leave; live jobs must first be joined or cancelled. `--color=auto`, `always`, and
`never` control generated color. Optional installation uses `meson install -C
build/release` and Meson's configured prefix.

## What works today

- Literal commands, byte pipelines, ordered redirection, and reusable job plans.
- Immutable bindings, lists, String/Bytes/Path values, and first-class native functions.
- Foreground/background processes, stop/resume, cancellation, and terminal restoration.
- A canonical-input REPL, strict UTF-8 source, and RGB/256/16-color or plain output.

Closures, currying, proper tail calls, records, ADTs and pattern matching are the next
language milestone. Structured value streams and JSON bridges follow; grapheme-aware
editing, completion, and persistent history complete the interactive milestone. The
[current status](docs/status.md) distinguishes usable features from these contracts.

## Explore the project

| Read                                                              | Purpose                                              |
| ----------------------------------------------------------------- | ---------------------------------------------------- |
| [Development](docs/development.md)                                | Build profiles, code conventions, and quality checks |
| [Architecture](docs/architecture.md)                              | Components, ownership, GC, and scheduling            |
| [Implementation plan](docs/implementation-plan.md)                | Repository layout and four acceptance milestones     |
| [Language](docs/language.md) · [Execution](docs/execution.md)     | First-release semantics and library contracts        |
| [Interaction](docs/interaction.md) · [Platform](docs/platform.md) | Input experience and Linux/macOS conventions         |
| [Testing](docs/testing.md)                                        | Test infrastructure and acceptance contracts         |
| [References](docs/references.md)                                  | Standards and upstream documentation                 |

Contributions should preserve explicit ownership and keep process bytes separate from
language values and display text. Start with the development guide and the contract
for the component you are changing.

## License

Rill Shell is licensed under the [MIT License](LICENSE). Unicode data retains its
[Unicode license](data/unicode/license.txt); yyjson retains its upstream MIT notice.
