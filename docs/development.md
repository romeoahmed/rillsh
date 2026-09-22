# Development

Use current stable Rust and the committed lockfiles. `rust-toolchain.toml` selects
stable with rustfmt and Clippy. Default `cargo build` and `cargo run` select the CLI;
`--workspace` includes every crate. Run locally; native Linux validation belongs in CI.

## Quality gate

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --release --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo bench --workspace --no-run --locked
```

Use `cargo test -p CRATE` for focused checks and `cargo bench --workspace -- --test`
for benchmark smoke tests. [Testing](testing.md) describes coverage and fixture policy.
The [grammar](../crates/tree-sitter-rill/README.md) and [fuzz](../fuzz/README.md)
packages own their regeneration and instrumentation gates.

The test profile inherits dev. Property/snapshot engines and their RNG/diff dependencies
use the optimization recommended by [Proptest](https://proptest-rs.github.io/proptest/proptest/tips-and-best-practices.html)
and [Insta](https://insta.rs/docs/quickstart/#optional-faster-runs); project code retains
debug checks. Release and bench use Cargo defaults. Change LTO, codegen units or
optimization levels only with representative measurements. Preserve unwinding for
owned-resource fallback cleanup.

## Code and dependencies

Keep dependency versions and internal paths in `workspace.dependencies`. Owning crates
enable their required features; check affected crates individually to catch accidental
feature-unification dependencies. Commit lockfiles and review licenses when upgrading.
Reedline owns editing and history; Tree-sitter owns generated parser artifacts. Use
upstream APIs and generators instead of patching dependencies.

Workspace Clippy enables `all`, `pedantic` and `nursery`. Fix findings. A scoped
`#[expect]` needs a demonstrated false positive and a reason; stale expectations fail.
Exact language Float equality and intentionally non-Send coordinator futures are valid
exceptions. Worker inputs and results must remain owned and `Send`.

[Architecture](architecture.md) owns the GC, scheduling and cleanup invariants. Keep
unsafe POSIX operations in `rill-system::sys`, with safety arguments for each boundary.
The PTY fixture has a narrow async-signal-safe pre-exec callback. Validate at the layer
that owns the input; carry guarantees in types instead of repeating checks downstream.

## Measurements

Criterion workloads live in their owning crates. Their names identify the measured
boundary:

- Parsing includes grammar construction and AST release.
- Prepared VM, equality, constructor and record-pattern workloads use
  `iter_batched_ref` to exclude setup and engine destruction. Source-to-result includes
  parsing, initialization and destruction.
- VM work includes quanta and GC pacing. Allocation workloads cover discarded cycles
  and scalar materialization. Large fixtures use `BatchSize::LargeInput`.
- JSON encoding uses prepared records; source-to-result pipelines also include data
  construction and decoding.
- Completion measures lexical context, local scope recovery or prepared metadata.
  CLI pipelines include startup, embedded modules, I/O and cleanup; filesystem setup
  is untimed.

For a short measurement:

```sh
cargo bench -p rillsh --bench pipelines --locked -- --sample-size 10 --warm-up-time 1 --measurement-time 2
```

Measure without concurrent builds, tests or fuzzing. Record compiler, profile, input and
sample settings under an ignored build directory. Keep timing thresholds out of tests;
shared CI runners provide smoke evidence, not reliable performance rankings.

## Documentation

README introduces the shell and gets a user started. Specifications own observable
behavior; architecture explains implementation. Link to the owning contract instead of
copying it. Preserve readable pipeline layout in Rill examples and check changed examples
manually; do not parse Markdown as test input.

Use `//!` for a module's purpose and ownership boundary, `///` for an item's observable
contract, and `//` for non-obvious invariants or ordering. Document errors, cancellation,
ownership and safety where relevant; omit empty template sections and repeated type
signatures. Prefer intra-doc links and executable rustdoc examples. Keep prose in English
and personal paths or host-specific setup out of tracked files.

## CI

CI checks pull requests and pushes to `main`; newer runs cancel superseded work.
Linux and macOS run Clippy, debug/release tests, benchmark builds and installation
checks. Formatting and rustdoc run once on Linux. Grammar and fuzz jobs run independently.

Caches stay separate by job and platform; only `main` updates them. Grammar caches its
installed CLI, while fuzz installs AFL and its runtime together. Fuzz results stay outside
build caches and upload as `fuzz-findings.tar.gz`, retained for seven days. Cancelled runs skip
uploads. Release publishing is not configured.
