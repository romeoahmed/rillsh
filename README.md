# Rill Shell

A shell for explicit pipelines, reusable job plans, and functional composition. Pass
arguments intact, build commands as values, and control each execution through a job
handle. First-class curried functions, immutable records, and pattern matching provide a
small language for composing that work.

`rillsh` runs on Linux and macOS and uses its own syntax. Command execution, job
control, and the functional language are available now. Structured streams and a rich
expression editor are planned; the current prompt uses the terminal's canonical editing.

## A first look

Run an external command with `^` and connect processes with `|`:

```rill
let greeting = "hello world"
^printf "%s\n" $greeting | ^cat
```

`$greeting` supplies one argument. There is no implicit word splitting, wildcard
expansion, or interpolation inside quoted strings.

Build a plan once and launch it repeatedly:

```rill
let greeting = job { ^printf "%s\n" "hello" }
run(greeting)
run(greeting)
```

A plan stores evaluated arguments. Its processes start and redirection files open only
when launched. Each launch creates a separate job and snapshots the current environment.

Start work in the background and check its completion:

```rill
let task = start(job { ^sleep 1 })
check(wait(task))
```

`wait` returns a report; `check` raises an error if it failed. Use `fg`, `bg`, and
`cancel` for job control. Scripts must acknowledge background jobs before reaching EOF.

Compose functions with `|>` and reuse partially applied functions:

```rill
fn larger_than(limit, value) => value > limit
let sizes = [128, 1024, 4096]
let total = sizes |> filter(larger_than(1000)) |> sum
^printf "%s bytes\n" $(text(total))
```

This prints `5120 bytes`. Functions capture lexical bindings; proper tail calls support
recursive composition. Records and nominal data types share nested pattern matching.
Arithmetic and conversions report errors instead of silently overflowing or coercing.

## Build and try it

Use Linux or macOS, GCC or Clang with GNU C23 and `<stdckdint.h>`, Meson 1.12+, Ninja,
and Python 3.14+. Python is a development dependency; the installed shell does not need
it.

```sh
meson subprojects download
meson setup build/release --buildtype=release --wrap-mode=nodownload
meson compile -C build/release
meson test -C build/release --print-errorlogs
build/release/src/rillsh
```

The first command downloads pinned yyjson source. Configuration and compilation then
need no network access. Select a compiler with `CC=gcc` or `CC=clang` at setup. The
optional [Containerfile](Containerfile) provides a Fedora development environment.

From your existing shell:

```sh
build/release/src/rillsh -c '^printf "%s\n" "hello"'
build/release/src/rillsh example.rill
build/release/src/rillsh --help
```

Save a Rill example above as `example.rill` before running the file command. Scripts
receive their arguments as Bytes through `args()`. Optional installation uses `meson
install -C build/release` with Meson's configured prefix.

At the prompt, incomplete expressions continue on the next line. Enter `exit(0)` or
press Ctrl-D at an empty prompt to leave; live jobs must first finish or be cancelled.
Interactive startup reads `rillsh/init.rill` under the XDG configuration directory;
`--no-config` skips it. `--color=auto`, `--color=always`, and `--color=never` select
RGB, palette, or plain output according to terminal capabilities and the override.

## Documentation

Start with [current status](docs/status.md) for implemented features and validation.
Structured streams, filesystem/JSON bridges, grapheme editing, completion, and
persistent history remain in the [implementation plan](docs/implementation-plan.md).

| Guide                                                             | Contents                                                           |
| ----------------------------------------------------------------- | ------------------------------------------------------------------ |
| [Language](docs/language.md)                                      | Values, functions, ADTs, patterns, errors, and modules             |
| [Execution](docs/execution.md)                                    | Commands, plans, jobs, and planned stream contracts                |
| [Interaction](docs/interaction.md) · [Platform](docs/platform.md) | Invocation, planned editing, and Linux/macOS conventions           |
| [Architecture](docs/architecture.md)                              | Components, ownership, evaluation, and memory                      |
| [Development](docs/development.md) · [Testing](docs/testing.md)   | Build profiles, contribution conventions, and acceptance contracts |
| [References](docs/references.md)                                  | Standards, upstream documentation, and design sources              |

For changes, read the development guide and the specification that owns the behavior.
Keep process bytes, language values, and display text separate; update the relevant
contracts and tests together.

## License

[MIT](LICENSE). Unicode data retains its [Unicode license](data/unicode/license.txt);
yyjson retains its upstream MIT notice.
