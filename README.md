# Rill Shell

A functional shell with structured data pipelines and reusable job plans

```rill
stream plan { ^printf "%s\n" "3" "1" "2" }
  |> lines
  |> map parse_int
  |> sort_by identity
  |> to_json
  |> write_bytes
```

Output: `[1,2,3]`. The command produces bytes; `lines` and `parse_int` turn them into
values. Ordinary functions sort those values, and JSON encoding returns bytes to stdout.

**Under development.** Rill has its own language and does not run POSIX shell scripts.
Its interactive editor supports multiline input, completion, persistent history and
color. A Tree-sitter grammar is available for editor integration.

## Why Rill

- **Explicit composition.** `|` connects process bytes; `|>` passes values to functions.
  Text, JSON, paths and bytes cross those boundaries through ordinary functions.
- **Reusable functions.** Currying, lexical closures, pattern matching, immutable
  records and proper tail calls support small, composable programs.
- **Reusable plans.** Describe a pipeline once, then run it, capture its output, stream
  its data or start it in the background. Inspect completion through structured reports.
- **Predictable arguments.** No implicit word splitting, globbing or interpolation
  inside quotes. Conversions and arithmetic failures are explicit.
- **Owned streams.** Lazy transforms respect demand; early termination closes upstream
  resources. Custom producers define acquisition, step and release callbacks.

## Build and try

Use current stable Rust on Linux or macOS. From the checkout:

```sh
cargo build --release --locked
target/release/rillsh
```

Run a command or script from your existing shell:

```sh
target/release/rillsh -c '^printf "%s\n" "hello"'
target/release/rillsh example.rill argument
target/release/rillsh --check example.rill
target/release/rillsh --format example.rill
target/release/rillsh --help
```

Install with `cargo install --path crates/rillsh --locked`. The standard library is
embedded; the executable does not need the checkout at runtime.

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
print (string total + " bytes")
```

This prints `5120 bytes`. Anonymous functions use `{ value => expression }`; patterns
work in function parameters, `let` and `match`.

A `plan` describes work without launching it. Every launch gets a fresh job and a snapshot
of the current environment:

```rill
let greeting = plan { ^printf "%s\n" "hello" }
run greeting
let output = capture greeting
write_bytes output.stdout
check output.report
```

Use `execute pipeline` for foreground I/O with a returned report. Use `start pipeline` and
`wait handle` for background work; `fg`, `bg` and `cancel` control jobs. Scripts must
wait for, foreground or cancel their background jobs before ending.

At the prompt, leave `|` or `|>` at line end to continue a pipeline. Enter submits
complete input; Alt+Enter inserts a newline. Ctrl+C cancels input or foreground work.
Ctrl+D on empty input requests exit and refuses while jobs remain live or stopped.
Ctrl+O opens the buffer in VISUAL or EDITOR; `help map` describes a function without
calling it. See [interaction](docs/interaction.md) for shortcuts, startup and history.

`read_file` and `write_file` connect byte pipelines to files; `flat_map` composes
sequential value streams.

Common operations are available directly. Module namespaces expose specialized helpers,
such as `seq.collect_with {max_items: 100}` and `text.byte_length`. `string` converts
scalars to String; `split_nul` decodes byte streams with NUL-delimited records.

## Develop and contribute

```sh
cargo test --workspace --locked
cargo doc --workspace --no-deps --open
```

Read the owning contract, update relevant tests and run the full
[quality gate](docs/development.md#quality-gate) before submitting a change. Bug reports
should include a minimal reproducer, expected and observed behavior, platform and Rust
version. Keep personal paths and sensitive environment data out of reports.

| Guide                                                             | Scope                                          |
| ----------------------------------------------------------------- | ---------------------------------------------- |
| [Language](docs/language.md)                                      | Values, functions, patterns and modules        |
| [Execution](docs/execution.md)                                    | Commands, streams, jobs and library operations |
| [Interaction](docs/interaction.md) · [Platform](docs/platform.md) | Invocation, editing and Unix contracts         |
| [Architecture](docs/architecture.md)                              | Crates, ownership, GC and scheduling           |
| [Development](docs/development.md) · [Testing](docs/testing.md)   | Build, code conventions and verification       |
| [References](docs/references.md)                                  | Standards and library documentation            |

[AGENTS.md](AGENTS.md) provides a concise guide for coding agents.

## License

[MIT](LICENSE). Dependencies retain their upstream licenses.
