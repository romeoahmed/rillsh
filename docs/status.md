# Implementation status

Rill Shell's Rust implementation includes:

- A bytecode runtime with closures, currying, recursion, tail calls, pattern matching
  and modules.
- Nine embedded standard-library modules for structured data, text, paths, JSON and
  stream composition.
- Process pipelines, redirection, capture, execution reports and background job control.
- Lazy streams, resource-owning producers, zip/merge and explicit cleanup across
  cancellation and stop/resume.
- Multiline editing, completion, history, color, tables and plain-terminal interaction.
- A Tree-sitter grammar, property and PTY tests, diagnostic snapshots, benchmarks and
  AFL++ targets.

## Verification

macOS debug/release tests and quality checks pass. Linux-target compilation and strict
Clippy pass; native Linux runtime validation and fuzz mutation campaigns remain pending.
Grammar regeneration/tests, fuzz seed replay, benchmark smoke checks and installation
outside the checkout have been verified. These checks do not establish performance
targets or exhaustive correctness.

## Boundaries

Scheduling is cooperative; operation limits are not global heap or wall-time bounds.
Reedline remains unmodified, so paste, undo and aggregate history memory follow its
native behavior.

See [architecture](architecture.md) for ownership and scheduling,
[interaction](interaction.md) for editor limits, and
[development](development.md#quality-gate) for verification commands.
