# Working on Rill Shell

Rill Shell is a Rust shell for Linux and macOS. Read [status](docs/status.md), the
[architecture](docs/architecture.md), and the owning [language](docs/language.md) or
[execution](docs/execution.md) contract before changing behavior. Preserve Rill syntax
and semantics; report deliberate behavioral changes explicitly.

## Build and verify

Use current stable Rust and the committed lockfiles:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The complete [quality gate](docs/development.md#quality-gate) adds release tests,
rustdoc and benchmark checks. Grammar and fuzz packages document their extra gates.
Report checks actually run and remaining gaps; local macOS results do not establish
Linux support.

## Change boundaries

- Keep dependency versions in `workspace.dependencies`; inherit required features in
  the owning crate. The isolated fuzz workspace owns its instrumentation dependencies.
- Keep traced references inside arena mutation; root every suspended continuation.
  Explicitly close resources, join workers and reap children. GC never runs OS cleanup.
- Validate inputs at their owning boundary and preserve guarantees in types. Delete
  duplicate checks and speculative fallbacks; retain explicit resource cleanup.
- Prefer library APIs and Rust ownership over handwritten infrastructure. Do not patch
  dependencies or edit generated Tree-sitter artifacts; regenerate through its CLI.
- Fix lint findings. Use a scoped `#[expect]` only for a demonstrated false positive,
  with its reason; stale expectations fail the build. Keep dependency configuration
  tied to actual API use and measured build or runtime needs.
- Write English rustdoc for API behavior, errors and ownership; explain implementation
  invariants with ordinary comments. Keep each behavior in one owning specification.
- Test values, effects and resource lifetimes with dedicated fixtures. Keep Markdown
  layout and examples outside the test inputs; rustdoc tests cover Rust API examples.
- Preserve unrelated work. Keep personal paths, host details and credentials out of
  tracked files. Use ignored build directories for local verification evidence.
