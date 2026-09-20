# Architecture

Rill combines a strict functional language with explicit process and stream ownership.
[Language](language.md), [execution](execution.md) and [interaction](interaction.md) own
observable behavior; [status](status.md) records evidence and gaps.

## Ownership boundaries

```text
rillsh ── rill-runtime ── rill-syntax
  │             └─────── rill-system
  ├────── rill-system
  └────── rill-editor ─── rill-syntax

tree-sitter-rill          Independent editor grammar
```

The six Cargo crates separate responsibilities, not deployment units. The application
owns scheduling; syntax and system services know nothing about language heap values. The
editor receives owned metadata snapshots and never accesses the VM. Finer divisions are
modules inside the owning crate. Each crate owns its tests and benchmarks. The separate
`fuzz/` workspace isolates instrumentation; the grammar follows Tree-sitter’s upstream
package layout.

- `rill-syntax` owns Logos lexical modes, Chumsky parsing, completeness, indexed ASTs,
  validation and byte spans. Commands and expressions have explicit lexical contexts.
- `rill-runtime` owns lowering, bytecode, lexical layouts, values, GC, modules, native
  primitives and stream continuations. Ordinary Rill modules live in its `stdlib/`.
- `rill-system` owns plans, launch snapshots, child groups, descriptors, directory
  capabilities, source transport, terminal state and the narrow POSIX unsafe boundary.
- `rill-editor` integrates Reedline validation, highlighting, completion, keys and
  native history. Library code owns Unicode editing, paste decoding and repainting.
- `rillsh` coordinates quanta, host requests, signals, notifications and foreground
  suspension. Its supervised helpers isolate filesystem traversal and child setup.
- `tree-sitter-rill` supplies incremental trees and queries for external tools. Its
  generated parser does not execute Rill or decide REPL completeness.

Validate language values when converting them into native requests. Checked ownership
types carry those guarantees across layers: native strings use `CString`, redirects
name stdout or stderr, and resources use owned descriptors. Downstream code should not
reconstruct and revalidate an already checked request. Libraries own their supported
editing and encoding behavior; the shell owns language contracts and resource cleanup.

## Language and heap

Lexing produces stable tokens before parsing; parser backtracking cannot change lexical
state. Tokens retain source byte spans and separation information. Lowering resolves
local reads to scope/slot indices, gathers constants and records explicit captures.
Instructions borrow immutable executable storage during dispatch; only retained operands
acquire ownership. Closures share immutable layouts across partial application. Every
callable uses the same unary protocol; tail calls replace frames rather than growing the
Rust stack.

`gc-arena` traces language values, lexical environments and active or parked VM state.
Derived tracing and mutation barriers preserve cycles. No borrowed arena value escapes a
mutation callback into an async operation, worker or editor channel. A VM quantum ends
before collection or host I/O. GC debt service is incremental. Known backing capacities
add pressure through the library's `adjust_debt` API; sampled debt checks can end a VM
quantum early. These weights are scheduling policy, not a hard pause or heap-byte bound.
Materialization and transport enforce their own budgets.

Lists use shared contiguous storage with suffix views; records retain insertion order
through IndexMap. Nominal construction checks descriptor names against the input map,
avoiding quadratic key scans, and stores payloads in declaration order. Record-rest
matching uses the existing key index to mark selected positions, then copies remaining
fields in insertion order. Structural equality ignores record order, distinguishes
nominal identities and validates hidden noncomparable leaves before mismatch shortcuts.
Its traced work state processes graph edges and long scalar buffers in bounded batches.
Union-find avoids repeatedly unfolding shared subgraphs. Hashing, allocation and GC
still have costs outside the instruction budget. JSON conversion, collection
construction and stable sorting also retain traced work state and spend VM fuel. JSON
uses borrowed Serde spans transiently and stores offsets across collection; encoding
keeps one child cursor per nesting level. Sorting uses the standard sorter for short
runs and checkpoints during stable merging. Scalar library calls and the bounded
retained-value accounting walk remain synchronous; no wall-clock preemption is promised.

Bindings publish transactionally at successful entry completion. Existing external
effects remain real when a later expression fails. A parked VM retains its lexical
snapshot. The active VM owns global publication and the nominal-name ledger; a traced
module cache and scope-identity allocator are shared with callback continuations.
Publication merges new declarations into current globals without restoring stale ones.

## Modules and standard library

`include_str!` embeds nine ordinary Rill modules. Native primitives expose
representation access, checked conversion, codecs and OS effects; Rill code owns
reusable composition, curried interfaces and public exports. The prelude selects
bindings without duplicating implementation or defining another callable model.

Bundled imports need no filesystem base. File imports retain directory capabilities and
opened file identities, so renames and aliases cannot silently change module identity.
Opening and reading are explicit host requests. The coordinator joins filesystem workers
before discarding their results. Parsing and compilation remain synchronous CPU work
outside OS waits. Cached identities, cycle checks and busy suspended initializers
prevent duplicate initialization.

## Process launch and I/O

A JobPlan contains evaluated arguments and redirects without launching anything. Each
launch receives an owned cwd descriptor and environment snapshot. The helper inherits
that environment, applies ordered redirects directly from borrowed arguments, and
replaces itself with the target executable. Helpers join the job's
process group before setup; gates prevent a partly prepared pipeline from executing. The
coordinator owns barrier progress, terminal handoff and cancellation. Every failure path
closes endpoints, cancels the owned group and reaps its children.

Tokio supplies signals and readiness on a current-thread runtime. `rustix` supplies
typed Unix APIs; its default backend selection is Linux `linux_raw` and libc on macOS.
Narrow libc calls cover POSIX facilities without a suitable rustix API. Workers carry
owned, Send inputs and results. Blocking work cannot be forcibly aborted through Tokio;
join it before returning to an editor or releasing its ownership scope.

Generational source handles identify OS resources. A separate traced stream token
records scope authority and single-consumer transfer. Process and directory sources
retain partial data and cancellation state outside the language heap. Bounded channels
and byte budgets provide backpressure; EOF is not successful process completion.
Terminal stderr from process streams uses the same cancellable writer protocol as
explicit output, including retained progress across stop/resume.

### Inherited output

An evaluation caches one idle writer per output stream. Concurrent callback writes own
separate active helpers and return a completed writer to that cache, reaping any
surplus. Payloads share owned `bytes::Bytes` storage across the VM/host boundary. A
frame is acknowledged only after its bytes reach the inherited descriptor. The
coordinator retains partial IPC progress across suspension; cancellation kills and reaps
the helper. This avoids changing shared descriptor flags or leaving a Tokio blocking
thread trapped behind pipe backpressure. Helpers close before their owner finishes and
move with parked evaluations. Explicit output remains byte-exact.

## Streams and cleanup

Lazy graph nodes retain transforms, source state and demand. Consumers retain explicit
actions across callbacks and host replies. `produce` callbacks run in traced child
scopes. Acquisition is lazy; release happens once after successful acquisition, with a
close reason. Cleanup continues after failure, preserving the first operational error
and attaching release failures. GC never invokes a language release callback.

`zip` holds one tuple and pulls left to right. `merge` preserves each input's order and
round-robin polls external readiness. Each language callback retains a traced VM stack
and spends the enclosing quantum's fuel. Source reads, process execution/capture, launch
readiness, file acquisition, output and job waits yield to ready peers. Owned operations
use generational readiness keys; errors route through nested merges to the requesting
callback. Foreground operations queue for one terminal lease, while captures and
independent I/O can progress. Completed callbacks transfer resource bookkeeping to their
owner. Cutoff discards ordinary work but finishes already-started release callbacks
before closing the graph. Cancel pending operations before running producer release so
abandoned I/O cannot race cleanup. Explicitly foregrounding a parked language evaluation
freezes its caller and transfers the foreground context; this is distinct from
independently waiting on an ordinary process job.

Suspension retains VM frames and host operations together, including capture buffers,
launch-gate positions and terminal modes. Stop barriers observe stopped or exited
children before editing resumes. `fg` restores the original continuation; cancellation
runs its cleanup rather than simply dropping the arena state.

## Interaction and verification

Rich editing uses Reedline's native capabilities with a fixed set of familiar shortcuts.
The execution parser decides completeness. Filesystem completion runs in one supervised
helper with bounded candidates and a deadline; accepting a filename inserts Rill source
that denotes one value or argument. Completion never evaluates user code.

Plain input uses a cancellable reader and the same parser. Ctrl+C ends the interrupted
display line and produces a fresh prompt. Ctrl+Z during editing restores terminal modes
and retains the buffer. History follows the native backend, including documented
reductions in [interaction](interaction.md); Rill does not patch the dependency.

Tests live beside their owning crates. Properties, reviewed diagnostic snapshots,
subprocess/PTY tests and Criterion workloads observe semantic and resource boundaries.
The independent AFL++ workspace and Tree-sitter regeneration use their upstream tools.
See [testing](testing.md) and the [quality gate](development.md#quality-gate).
