# Working on Rill Shell

Rill Shell is a Rust shell for Linux and macOS. Read the
[architecture](docs/architecture.md) and the owning [language](docs/language.md),
[execution](docs/execution.md) or [interaction](docs/interaction.md) contract before
changing behavior. Preserve Rill syntax and semantics; report deliberate changes.

## Verify

Use current stable Rust and the committed lockfiles:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The [quality gate](docs/development.md#quality-gate) adds release tests, rustdoc and
benchmark checks. Grammar and fuzz packages document their extra gates. Report checks
actually run and remaining gaps; local macOS results do not establish Linux support.

## Change boundaries

- Keep versions in `workspace.dependencies`; enable features in the owning crate.
  The isolated fuzz workspace owns its instrumentation dependencies.
- Keep traced references inside arena mutation and root suspended continuations.
  Explicitly close resources, join workers and reap children; GC never runs OS cleanup.
- Validate at the owning boundary, then carry guarantees in types. Prefer library APIs
  and Rust ownership; delete duplicate checks and speculative fallbacks.
- Do not patch dependencies or edit generated Tree-sitter artifacts. Regenerate through
  its CLI. Fix lints; use scoped `#[expect]` only for a demonstrated false positive.
- Test behavior and resource lifetimes with dedicated fixtures, never Markdown inputs.
  Keep contracts in one owning specification; use concise English rustdoc and comments
  for API behavior and implementation invariants.
- Preserve unrelated work. Keep personal paths, host details and credentials out of
  tracked files; store local verification evidence in ignored build directories.
