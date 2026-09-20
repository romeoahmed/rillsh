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

Output: `[1,2,3]`. `lines` and `parse_int` turn the command’s bytes into values;
ordinary functions sort them, then JSON encoding returns bytes to stdout.

**Under development.** The interactive shell includes multiline editing, completion and
persistent history; a Tree-sitter grammar supports editor integration. Rill has its own
syntax and does not run POSIX shell scripts. See [status](docs/status.md) for delivered
features and pending Linux validation.

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
  closes upstream resources. Custom producers own acquisition and release callbacks;
  materialization limits and producer failures are explicit.

## Build and try

Use current stable Rust on Linux or macOS. From the repository root:

```sh
cargo build --release --locked
cargo test --workspace --locked
target/release/rillsh
```

Run a command or a saved script from your existing shell:

```sh
target/release/rillsh -c '^printf "%s\n" "hello"'
target/release/rillsh example.rill argument
target/release/rillsh --help
```

Script arguments are Bytes available through `args ()`. Install from a checkout with
`cargo install --path crates/rillsh --locked`; the standard library is embedded. See
[development](docs/development.md) for the complete native quality gate.

## A small tour

External commands start with `^`. A substitution contributes one argument, even when it
contains spaces:

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

At the prompt, incomplete input continues on the next line. Leave `|` or `|>` at the end
of a line when entering a multiline pipeline. Ctrl+C cancels an entry or foreground
execution. During evaluation, Ctrl+Z retains the continuation and its jobs; use `jobs
()` to find its handle, then `fg handle` to resume or `cancel handle` to clean up. `bg`
resumes external-only jobs. Exit with `exit 0` or Ctrl+D on empty input after finishing
or cancelling active work.

Interactive startup reads `rillsh/init.rill` under the XDG configuration directory;
`--config FILE` selects another file and `--no-config` skips startup loading. Color
follows terminal capabilities and can be overridden with `--color=auto|always|never`.

## Explore and contribute

| Start here                                                        | What it covers                                       |
| ----------------------------------------------------------------- | ---------------------------------------------------- |
| [Language](docs/language.md)                                      | Functions, data, patterns, errors, and modules       |
| [Execution](docs/execution.md)                                    | Commands, streams, reports, filesystem, and JSON     |
| [Interaction](docs/interaction.md) · [Platform](docs/platform.md) | Invocation, editing, and platform contracts          |
| [Architecture](docs/architecture.md) · [Status](docs/status.md)   | Ownership, delivered features, and verification gaps |
| [Development](docs/development.md) · [Testing](docs/testing.md)   | Building, contributing, and verifying changes        |
| [References](docs/references.md)                                  | Standards and design sources                         |

For a change, read the owning specification, update the relevant tests, and run the
[quality gate](docs/development.md#quality-gate). Bug reports should include a minimal
reproducer, expected and observed behavior, and the platform and Rust version used.
Avoid personal paths or sensitive environment data. Browse Rust API documentation with
`cargo doc --workspace --no-deps --open`. [AGENTS.md](AGENTS.md) gives coding agents a
concise working guide.

## License

[MIT](LICENSE). Dependencies retain their upstream licenses.
