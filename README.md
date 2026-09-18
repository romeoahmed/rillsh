# Rill Shell

Rill Shell (`rillsh`) brings structured data and functional composition to everyday
command-line work. Build reusable job plans, connect process bytes to value streams, and
compose first-class curried functions over immutable records and pattern matching.
Arguments stay intact; conversions and resource limits are explicit.

It runs on Linux and macOS with its own syntax. The language, pipelines, streams, and
job control are implemented. The prompt currently uses canonical terminal input;
grapheme editing, completion, and persistent history are the next milestone. See
[current status](docs/status.md) for coverage and limitations.

## Commands, values, and reusable plans

Run external programs with `^`; connect their byte streams with `|`:

```rill
let message = "hello world"
^printf "%s\n" $message | ^cat
```

`$message` contributes exactly one argument. There is no implicit word splitting,
wildcard expansion, or interpolation inside quoted strings.

Use `|>` to pass a value to a function. Configuration comes first, so partial
application fits naturally into pipelines:

```rill
fn larger_than(limit, value) => value > limit
let total = [128, 1024, 4096] |> filter(larger_than(1000)) |> sum
^printf "%s bytes\n" $(text(total))
```

This prints `5120 bytes`. Functions capture lexical bindings and support proper tail
calls. Records, nominal types, and nested patterns share the same data model. Arithmetic
reports overflow and invalid conversions rather than silently coercing.

A `job` expression builds a reusable plan without starting its processes:

```rill
let numbers = job { ^printf "%s\n" "3" "1" "2" }
run(numbers)
stream(numbers) |> lines |> map(from_json) |> sort_by(identity)
  |> to_json |> chunks |> write_stdout
```

The first launch prints three lines; the second writes `[1,2,3]`. `lines` and JSON
conversion make the byte/value boundary explicit. Streams are lazy and single-consumer;
`take` closes unneeded upstream work, and sinks check producer completion. Each launch
creates its own job and snapshots the current environment.

For background work, use `start(plan)` and then `check(wait(handle))`. `fg`, `bg`, and
`cancel` provide job control. Ctrl-Z can preserve an active foreground evaluation for
`fg`; background execution is limited to external jobs. Scripts must acknowledge jobs
before EOF. [Execution](docs/execution.md) covers these lifetimes and failure policies.

## Build and try

You need Linux or macOS, GCC or Clang with GNU C23 and `<stdckdint.h>`, Meson 1.12+,
Ninja, and Python 3.14+. Python is only a development dependency.

```sh
meson subprojects download
meson setup build/release --buildtype=release --wrap-mode=nodownload
meson compile -C build/release
meson test -C build/release --print-errorlogs
build/release/src/rillsh
```

The first command acquires pinned yyjson source; subsequent configuration and
compilation need no network access. Set `CC=gcc` or `CC=clang` at initial setup to
choose the compiler. An optional [Containerfile](Containerfile) provides Fedora tools.

From your existing shell:

```sh
build/release/src/rillsh -c '^printf "%s\n" "hello"'
build/release/src/rillsh --help
```

Save a Rill example as `example.rill` and run `build/release/src/rillsh example.rill`.
Scripts receive file arguments as Bytes through `args()`. Install with `meson install -C
build/release` using Meson's configured prefix.

At the prompt, incomplete expressions continue on the next line. For a multiline
pipeline, leave `|` or `|>` at the end of the line before pressing Enter. Enter
`exit(0)` or press Ctrl-D on empty input to leave; live jobs must finish or be
cancelled. Interactive startup reads `rillsh/init.rill` under the XDG configuration
directory; `--no-config` skips it. Color follows terminal capability, with
`--color=auto`, `--color=always`, and `--color=never` overrides.

## Read more and contribute

| Guide | Purpose |
| --- | --- |
| [Language](docs/language.md) | Values, functions, ADTs, patterns, errors, and modules |
| [Execution](docs/execution.md) | Commands, plans, jobs, streams, filesystem, and JSON |
| [Interaction](docs/interaction.md) · [Platform](docs/platform.md) | Invocation, planned editing, and Linux/macOS conventions |
| [Architecture](docs/architecture.md) · [Implementation plan](docs/implementation-plan.md) | Ownership, components, and remaining milestones |
| [Development](docs/development.md) · [Testing](docs/testing.md) | Build profiles, code conventions, and acceptance contracts |
| [References](docs/references.md) | Standards and design sources |

For a change, start with the development guide and the specification owning the
behavior. Update its contract and relevant tests together. Code annotations generate a
Doxygen reference through Meson's `api-docs` target; [AGENTS.md](AGENTS.md) provides
concise instructions for coding agents.

## License

[MIT](LICENSE). Unicode data retains its [Unicode license](data/unicode/license.txt);
yyjson retains its upstream MIT notice.
