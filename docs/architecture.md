# Architecture

Rill combines a strict functional language with explicit process and stream ownership.
[Language](language.md), [execution](execution.md) and [interaction](interaction.md)
define observable behavior; this document explains the implementation boundaries.

## Crates

```text
rillsh ── rill-runtime ── rill-syntax
  │             └─────── rill-system
  ├────── rill-system
  └────── rill-editor ─── rill-syntax

tree-sitter-rill          Independent editor grammar
```

| Crate              | Owns                                                                                                     |
| ------------------ | -------------------------------------------------------------------------------------------------------- |
| `rill-syntax`      | Logos contexts, Chumsky parsing, completeness, indexed ASTs, validation and byte spans                   |
| `rill-runtime`     | Lowering, bytecode, values, GC, modules, native primitives and stream continuations                      |
| `rill-system`      | Plans, launch snapshots, process groups, descriptors, transport, terminals and POSIX adapters            |
| `rill-editor`      | Reedline integration: validation, highlighting, completion, keys and native history                      |
| `rillsh`           | Scheduling, host effects, signals, notifications, supervised helpers and foreground suspension           |
| `tree-sitter-rill` | Generated incremental parser and queries for external tools; no execution or REPL completeness decisions |

These are responsibility boundaries, not separate deployments. Finer divisions remain
modules; tests and benchmarks live with their owning crate. The independent `fuzz/`
workspace isolates instrumentation. Grammar files follow Tree-sitter's package layout.

Syntax and system services know nothing about language heap values. The editor receives
owned metadata snapshots. Validate values when converting them into host requests, then
carry guarantees in types: `CString` for native strings, explicit redirect targets and
owned descriptors. Downstream layers do not repeat that validation.

## Evaluation and GC

Lexing fixes command/expression contexts before parser backtracking. Lowering resolves
local reads to scope/slot indices, records constants and selects closure captures.
Dispatch borrows immutable code; retained operands acquire ownership. Partial
applications share layouts, every callable uses the unary protocol, and tail calls
replace frames rather than growing the Rust stack.

`gc-arena` traces values, environments and active or parked VM state. Derived tracing
and mutation barriers preserve cycles. Arena borrows never escape into async operations,
workers or editor channels. Quanta end before collection or host I/O. Incremental debt
service uses the library's pacing; backing capacities add pressure through `adjust_debt`.
Sampled debt checks can yield early, including inside allocating collection and JSON
decode loops. These weights are neither heap limits nor pause-time guarantees.

Lists use shared contiguous storage and suffix views. IndexMap preserves record field
order. Nominal construction indexes declared keys; record-rest matching marks selected
positions and copies unmatched fields. Equality ignores record order, preserves nominal
identity and checks hidden noncomparable leaves before mismatch shortcuts. Its traced
work state batches edges and scalar buffers; union-find avoids repeatedly unfolding
shared graphs.

JSON conversion, collection construction and stable sorting also retain traced state
and spend VM fuel. JSON keeps offsets into input across collection and one encoding
cursor per nesting level. Sorting uses the standard sorter for short runs and checkpoints
stable merges. Scalar primitives and retained-value accounting remain synchronous;
instruction fuel does not bound hashing, allocation or wall time.

Entry bindings publish transactionally; prior external effects are not rolled back.
Parked VMs retain lexical snapshots. Publication merges new declarations into current
globals without restoring stale bindings. Callback continuations share a traced module
cache and scope-identity allocator; the active VM owns global publication and nominal
name checks.

## Modules

`include_str!` embeds the standard library. Native primitives provide representation
access, checked conversions, codecs and OS effects. Rill modules own composition,
curried interfaces and exports; the prelude selects public bindings.

Bundled imports need no filesystem base. File imports retain directory capabilities and
opened file identities across renames. Opening and reading are host requests; workers
are joined before results are discarded. Parsing and lowering remain synchronous CPU
work. Cached identities, cycle checks and busy suspended initializers prevent duplicate
initialization.

## Process launch and I/O

A plan contains validated arguments and redirects. Each launch snapshots cwd and
environment. Helpers join the job group before setup, apply ordered redirects and exec
the target. Gates hold every target until the pipeline is ready. The coordinator owns
barrier progress, terminal handoff and cancellation; failures close endpoints, cancel
owned groups and reap children.

Tokio provides signals and readiness on a current-thread runtime. rustix selects
`linux_raw` on supported Linux targets and libc on macOS; narrow libc calls cover
facilities without a suitable API. Workers carry owned `Send` data. Blocking work must
be joined because Tokio cannot forcibly abort it.

Generational source handles identify OS resources. Traced stream tokens separately
record scope authority and single-consumer transfer. Partial transport and cancellation
state live outside the heap. Bounded channels and byte budgets provide backpressure;
EOF does not prove successful process completion.

### Inherited output

An evaluation caches one idle writer per output stream. Concurrent callbacks use
separate active helpers, returning completed writers to the cache and reaping surplus
helpers. Shared `bytes::Bytes` payloads cross the VM/host boundary. A frame is acknowledged
only after reaching the inherited descriptor; partial IPC progress survives suspension.

Cancellation kills and reaps the helper. This avoids changing shared descriptor flags
or trapping a blocking thread behind pipe backpressure. Writers move with parked
evaluations and close before their owner finishes. Relayed terminal stderr uses the
same protocol. Explicit output remains byte-exact.

## Streams, cleanup and suspension

Lazy nodes retain transforms, source state and demand. Consumers keep explicit actions
across callbacks and host replies. Producer callbacks run in traced child scopes;
cleanup continues after failure and attaches release errors to the original error.
GC never releases OS resources or invokes language cleanup.

`zip` holds one tuple and pulls left to right. `merge` polls external readiness
round-robin while preserving per-input order. Each language callback retains a VM stack
and spends the enclosing quantum's fuel. Source reads, launches, capture, file work,
output and job waits yield to ready peers. Generational operation keys route replies
and failures through nested merges. Foreground operations share one terminal lease.

Completed callbacks transfer resource bookkeeping to their owner. Cutoff finishes
already-started release callbacks before closing the graph. Cancel abandoned I/O before
running release so it cannot race cleanup. `Engine::interrupt` and `Engine::finish`
are driven until finalization completes; starting another entry cannot bypass it.

Suspension retains frames, host operations, capture buffers, launch-gate progress and
terminal modes. Stop barriers observe stopped or exited children before returning to
editing. `fg` resumes that continuation; cancellation runs its cleanup. Foregrounding a
parked language evaluation freezes its caller too, unlike independently waiting for an
ordinary process job.

## Editor boundary

Reedline owns editing and repainting; the execution parser owns completeness. Completion
never evaluates code. Filesystem completion uses one supervised helper with bounded
results and a deadline; replies are tied to buffer/cursor origins and discarded when
superseded. Insertion denotes exactly one Rill value or command argument.

Plain input uses a cancellable reader and the same parser. Cancellation ends the display
line before a fresh prompt; editing suspension restores modes and retains the buffer.
Native history and editor limits are specified in [interaction](interaction.md).
