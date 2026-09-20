# Development

This guide owns build, implementation, and documentation conventions. [Current
status](status.md) identifies the available feature set; [testing](testing.md) defines
behavior and acceptance evidence.

## Quality gate

Use current stable Rust and the committed lockfile. `rust-toolchain.toml` selects stable
with rustfmt and Clippy. Run locally; native Linux validation belongs in CI. Default
`cargo build` and `cargo run` select the CLI; use `--workspace` for every crate.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --release --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo bench --workspace --no-run --locked
```

Use `cargo test -p CRATE` to select an affected crate. The test profile inherits dev;
only the property/snapshot engines and their RNG/diff dependencies use higher dev
optimization, following
[Proptest](https://proptest-rs.github.io/proptest/proptest/tips-and-best-practices.html)
and [Insta](https://insta.rs/docs/quickstart/#optional-faster-runs). Project code
retains normal debug information and checks. Release and bench use Cargo defaults;
change LTO, codegen units or optimization levels only after representative measurements,
not by assuming that more optimization is always faster. Preserve unwinding for
owned-resource fallback cleanup.

Criterion workloads live in their owning crates. `cargo bench --workspace` measures
them; `-- --test` runs a smoke check. Each workload names its boundary:

- Parsing includes grammar construction and AST release.
- Prepared evaluation, equality, constructors and record matching exclude fixture
  setup and engine destruction through `iter_batched_ref`. Source-to-result includes
  parsing, initialization and destruction through `iter`. Equality workloads include
  equal values and early/late mismatches.
- VM work includes quanta and GC pacing. Large fixtures use `BatchSize::LargeInput`
  to limit simultaneously live engines. Pure workloads reject unexpected host requests.
- Completion measures lexical context or prepared name/record metadata.
- CLI pipelines include startup, embedded modules, I/O and cleanup; filesystem fixture
  creation is untimed.

For a short process-pipeline measurement:

```sh
cargo bench -p rillsh --bench pipelines --locked -- --sample-size 10 --warm-up-time 1 --measurement-time 2
```

Run performance measurements without concurrent builds, tests or fuzzing. Keep timing
thresholds out of correctness tests. See the [grammar
package](../crates/tree-sitter-rill/README.md) for regeneration/corpus commands and
[fuzzing](../fuzz/README.md) for instrumented targets. The grammar uses generated C
through `cc`; normal Cargo builds need no grammar generator. The fuzz workspace has its
own manifest, lockfile and lint gate.

## Implementation

Workspace lints enable Clippy `all`, `pedantic`, and `nursery`. Fix findings rather than
suppress groups. Exact language Float equality has a scoped, explained `float_cmp`
expectation; stale expectations fail. `future_not_send` expectations are confined to the
current-thread coordinator: traced state intentionally cannot migrate to a worker. All
actual worker inputs and results remain owned and `Send`. Keep unsafe POSIX boundaries
in `rill-system::sys`, with owned inputs and documented safety invariants. The PTY
fixture has a narrow async-signal-safe pre-exec callback. GC state remains rooted
between quanta; collection and host I/O happen outside mutation callbacks. Explicit
shutdown joins workers and reaps children; Drop is a fallback, not the asynchronous
cleanup protocol. Producer callbacks run in traced child scopes. Drive
`Engine::interrupt` or `Engine::finish` until finalization completes. Beginning another
entry cannot discard active work, and there is no public abort path that bypasses
release callbacks. Preserve the original error and source location while attaching
cleanup failures. Keep lexical layouts local to compiled code and immutable after
lowering. Frame-local reads use scope/slot indices; closures retain selected values and
share layouts across partial applications. Borrow pattern names during matching and
publish bindings only after a complete match. Entry publication merges declarations, not
a stale global snapshot.

## Tests and reviewed outputs

Use native Rust unit and owning-crate integration tests. Keep semantic assertions on
values, errors, effect order and owned-resource lifetimes. Executable and PTY fixtures
exercise the actual binary with explicit shutdown and deadlines. Do not skip a missing
PTY capability or use sleeps as readiness evidence.

Diagnostic snapshots use insta. Review the source label, Unicode location and cleanup
notes before accepting a change; never enable automatic snapshot updates in CI. Proptest
regression files and minimized fuzz inputs are reviewed source artifacts.

## Documentation

Use `//!` for a crate or module's purpose and ownership boundary; use `///` for an
item's observable contract. Start with a useful summary, then document non-obvious
ownership, cancellation and `# Errors`, `# Panics` or `# Safety` requirements. Link to
Rust items with intra-doc links. Small Rust API examples should run as rustdoc tests. Do
not repeat type signatures or add empty documentation sections to satisfy a template.

Ordinary `//` comments explain invariants, ordering or a reason the code alone cannot
show. Safety comments justify the specific unsafe operation and its borrowed lifetimes.
Keep English prose concise and update it with the implementation.

README introduces the shell with useful examples and a short build path. Specifications
own Rill behavior; architecture explains mechanisms; status records delivered work and
verification gaps. Link instead of copying contracts between them. Manually check
changed Rill examples, preserving readable pipeline layout; do not make tests parse
Markdown. Keep personal paths and host-specific setup out of tracked files.

## Dependencies and CI

Keep versions and internal paths in root `workspace.dependencies`. Each owning crate
inherits its dependencies and enables the features it uses. Check affected crates
individually as well as together to catch accidental feature-unification dependencies.
Use released dependencies with Cargo's native feature unification and committed
lockfiles. Reedline owns editing and history; its unused optional editing modes stay
disabled. Tree-sitter owns generated C and binding conventions. Use Cargo for
application builds and the upstream CLI for grammar generation. Review upstream licenses
when upgrading dependencies.

CI checks pull requests and pushes to `main`, avoiding duplicate branch-push checks for
open pull requests. A newer run cancels superseded work for the same PR or branch.
Platform, grammar and fuzz jobs use separate gates; release publishing is not
configured.
