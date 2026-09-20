# Implementation status

## Delivered capabilities

Cargo builds the shell, embedded standard library and independently usable grammar.
Implementation and platform evidence are separate below.

| Component           | Delivered                                                                                                                                                                                                                              |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Syntax              | Logos command/expression modes, Chumsky grammar, indexed AST, validation, byte spans and parser-backed completeness                                                                                                                    |
| Runtime             | Bytecode, lexical slots, explicit captures, unary currying, recursion, proper tail calls, nominal matching, checked arithmetic, checkpointed graph equality, shared module identities, transactional publication and traced suspension |
| Library and modules | Nine embedded Rill modules, explicit exports, Option/Result composition, text/bytes/paths, strict JSON, sorting, file-identity cache, cycle/busy checks and explicit module I/O requests                                               |
| Unix execution      | Gated process groups, launch snapshots, redirects, execution/capture/reports, background jobs, stopped evaluations, wait/fg/bg/cancel, owned transport and explicit reaping                                                            |
| Streams             | Single-consumer scopes, lazy transforms, unfold/range, short-circuit folds, bounded collection, lines, process/directory/stdin sources, zip/merge, through and resource-owning producers with traced release                           |
| Interaction         | Multiline validation, name/field/path completion, signatures, bounded materialized tables, native history, color, plain fallback, Ctrl+C/Ctrl+D, editing suspension, incremental stream display and repaint-aware notifications        |
| Tooling             | Tree-sitter grammar, queries and incremental tests; properties and reviewed diagnostic snapshots; Criterion; isolated cargo-afl targets and regression seeds                                                                           |

## Runtime boundary

The coordinator retains VM state and owned operations across waits and stop/resume. GC,
conversion, equality and sorting use cooperative checkpoints; allocation, scalar
operations and filesystem workers can still take unbounded wall time. Representation
budgets do not measure RSS or impose a global heap ceiling. See
[architecture](architecture.md) for ownership and scheduling.

## Verification boundary

The native Linux/macOS workflow is configured. Local macOS verification does not
establish Linux support; remote Linux execution remains pending. Native macOS debug and
release gates cover the language, resources, CLI and controlling-PTY interaction, with
no ignored tests. Workspace and isolated-fuzz Clippy pass with warnings denied;
formatting and rustdoc also pass. All benchmark targets build. Tree-sitter regeneration
is byte-identical, its corpus and Rust query/incremental tests pass, and all 23 seeds
across six instrumented AFL++ targets replay successfully. A clean source export
installs and imports the embedded library from outside its checkout.

No successful local mutation campaign is claimed. Native Linux CI owns bounded mutation
campaigns; seed replay and workflow configuration are not campaign or Linux execution
evidence.

## Interaction boundary

Reedline is unmodified. Rill enforces submitted-source, completion and history-entry
limits, but does not bound the editor's paste, undo or aggregate history memory.
[Interaction](interaction.md) specifies these limits, history exclusions and the
separate plain-input behavior. Completion never evaluates user code; automatic display
finishes stream cleanup before publication. Startup reads only the user XDG config
location, never `XDG_CONFIG_DIRS`; `--config FILE` selects an explicit startup file.
