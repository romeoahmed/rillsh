# Architecture

Rill Shell combines a strict functional language with explicit process and stream
ownership. This document explains how the components preserve those semantics.
[Language](language.md), [execution](execution.md), and [interaction](interaction.md)
own user-visible behavior; [status](status.md) distinguishes implemented features from
planned stream extensions and editor work. [The implementation plan](implementation-plan.md) maps these
boundaries to files and milestones.

## Design principles

Keep the language core small: unary application, lexical bindings, immutable data,
pattern matching, and explicit errors. Standard-library functions express composition;
native primitives provide representation access and OS effects. Parsing, completion, and
presentation never execute user code.

One thread owns mutable runtime and session state. Cooperative steps preserve effect
order while allowing signal, job, and I/O progress. Garbage collection owns language
storage; explicit scopes own descriptors, directories, and child processes.

The first release excludes POSIX-shell syntax compatibility, implicit expansion, mutable
bindings, classes, macros, static typing, native plugins, a package manager, JIT
compilation, Windows support, and configurable editor modes.

## Cohesive components

```text
                        session
                    /      |      \
               native     exec    editor (planned)
                 |         |         |
              runtime   platform   syntax
                 |                   |
               syntax ------------- text
```

This is an orientation map; the [dependency
table](implementation-plan.md#module-boundaries) is authoritative. Components expose
internal ownership boundaries, not a public C ABI.

- `source` and `diagnostic` define shared text ownership and error views.
- `syntax` owns parsed trees, copied source, completeness, and grammar metadata.
- `runtime` owns Values, traced code, lexical bindings, GC, and continuations.
- `native` converts between language Values and concrete native services.
- `exec` supervises evaluated launch specifications without knowing ASTs or Values.
- `platform` contains POSIX mechanisms and observed Linux/macOS differences.
- `text` supplies byte buffers, Unicode operations, and styles.
- `session` coordinates effects, input, presentation, and suspended contexts.
- The planned `editor` consumes syntax results and materialized metadata, never an
  evaluator to invoke.

The JSON adapter isolates yyjson types and calls. Pure decimal parsing reuses its
number-token conversion through a Rill-only interface; the runtime and general
primitive dispatcher do not depend on yyjson representations.

Lower components return structured diagnostics. The session renders them and consumes
borrowed text before releasing its owner. Native state machines yield through the
existing evaluator protocol instead of running recursive evaluators or nested event
loops. Completion will use supervised helper processes, not a background user evaluator.

## Syntax and evaluation

Source is owned UTF-8 with a logical name and byte offsets. Presentation derives line
and display coordinates. Source NUL is invalid; a valid String escape may produce NUL
until an OS boundary rejects it.

The recursive-descent/precedence parser has explicit expression and command modes and a
256-level nesting limit. Quoted strings copy ordinary byte runs in bulk and decode
escapes separately. Record validation sorts borrowed key pointers and scans adjacent
keys; it never reorders fields or their effects. Comparisons include byte lengths and
embedded NUL. Stable typed node blocks own decoded text; parsed identifier tokens use
bounded `strndup` copies without growable-buffer slack. Clearing any parse result is
iterative. Operator spellings become enums during parsing. Code preparation
consumes and resets the parse result, including on failure, and assigns constant and
function-layout slots and aggregate frame capacities without executing user code.
These capacities reuse kind-specific node payload storage; no parallel IR or extra
per-node allocation is needed. Structural pattern errors are
rejected before execution. Nominal resolution and value mismatches remain runtime
checks. Closure headers and Record fields share brace syntax; lexical lookahead over
the header selects the form without parsing a body twice.
Application is a separate left-associative layer above prefix and infix operators.

Multi-parameter functions lower to unary functions. Recursive function expressions
lower to a private block containing a named recursive declaration and its value;
there is no second recursion mechanism. Export tables evaluate directly to immutable
namespaces instead of annotating declarations and rescanning a module afterward.
Module frames retain their own export value. During bootstrap, explicit module imports
share the loader cache; freezing the prelude installs only its exported bindings.
User functions, native functions, callbacks, and constructors share one application protocol. A value pipe evaluates its
left operand before the callable; whitespace application preserves each intermediate
unary application. A closure body is a lexical statement block; single-expression bodies
need no extra scope or continuation. Preparation must not reorder those effects.

The prepared-AST evaluator uses explicit continuation frames. Each frame roots its
scope, code, and initialized operand prefix. Atomic literals and name references write
directly into a parent's rooted slot; other expressions use the normal continuation
protocol. String literals read a prepared slot without allocation. Free-name references
inside functions use prepared capture slots, including constructor references in
patterns. Local scopes are traversed to the closure boundary without comparing names;
local and top-level references retain ordinary lexical lookup. The slot belongs to code,
while its value belongs to each closure instance. Recursive slots still read through
their cells. Preparation adds neither an AST node allocation nor a second IR.

Tail calls replace frames and discard obsolete operands promptly. Sequential forms use
fixed-size frames; aggregates read their precomputed operand capacity rather than
walking the child chain on every invocation. A bounded cache reuses
small inactive frames, while wide frames return to libc. Replacement can shrink a wide
frame, so tail recursion does not retain an unbounded high-water allocation. Non-tail
continuations are limited to 65,536.

An entry publishes bindings and nominal names together after validation. Failed entries
preserve completed external effects but publish no bindings. Abort clears pending work
and borrowed diagnostics before collection; committed bindings remain rooted.

## Values and memory

Values use a tagged C union with immediate scalars or nonmoving object pointers. There
is no NaN boxing, packed layout, or pointer tagging. Each object has collector links,
kind/mark state, allocation size, and an initialized edge count. Its kind selects a
union for text, code, capture layouts, Record indexes, budgets, or stage policy. Values,
optional indexes, and inline bytes follow the header in the same allocation.
Compile-time assertions protect interior alignment; sizes are not a public ABI.

### Data and code ownership

| Representation     | Ownership and access                                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| List/slice         | Immutable backing array; a suffix view retains that array, a full view reuses its input, and an empty view has no backing edge |
| Record             | Unique length-aware String keys in presentation order; wider Records add an interior sorted key index                          |
| Nominal value      | Unique descriptor and immutable payload; fieldless constructors are singleton values                                           |
| Closure            | One code edge followed by exact resolved captures; layout names borrow the retained code                                       |
| Code               | Owned source, syntax blocks, capture-analysis layouts, and a traced String constant pool                                       |
| Committed bindings | One immutable snapshot with Values, name index, and copied names; no edge to the replaced snapshot                             |

Private aggregate builders begin with initialized Unit slots. Root the owner before
allocating children and expose only initialized slots to GC. Finalize Record keys and
indexes before publication. A copied finalized Record relocates its interior index;
changing values preserves it, while changing keys or count requires finalization. Small
Records use linear lookup; wider ones use binary search over an index sorted by libc.
Indexed immutable updates take `O(n + k log n)` field work for `k` replacements, plus
key-byte comparisons.

List-rest matching shares backing storage; a tiny retained slice can therefore keep a
large array alive. Ignored rest patterns create no suffix or remainder Record. Language
immutability does not prohibit writes to unpublished builders or recursive binding
cells.

Each function has one code-owned free-name summary. An enclosing function consumes
nested summaries instead of rewalking nested bodies. Each closure instance resolves its
own bindings, retaining recursive cells rather than their current contents. Exact
captures prevent unrelated resources or replaced bindings from extending lifetimes.

Equal String literals and Record keys share code-local constant slots. Temporary node
pointers are sorted by length-aware bytes; there is no global intern table. String nodes
release redundant decoded buffers, while Record keys retain text needed by matching.
Constants have no back edge to code, so an escaped String can survive after its syntax
is collected. Descriptors, closures, and mutable builders are never interned.
Preparation completes and charges all retained metadata before code publication.

Local scopes use binding chains. A closure itself terminates a parameter-binding scope,
so all parameter patterns share the same matching path without an empty environment
allocation. Blocks still introduce boundaries; duplicate checks and nominal descriptor
resolution keep their original lexical scope. Publication reads the pending chain up
to its entry boundary and the current committed environment directly, without cloning
bindings into an intermediate GC chain. It sorts names with pending-entry precedence
and recency tie-breaking, then merges with the committed snapshot. A resumed entry
therefore preserves intervening unrelated definitions while publishing its own names. For `k`
pending and `n` committed names, comparison work is `O(k log k + n)`, with `O(n + k)`
scratch plus name bytes. An entry without bindings reuses the old snapshot. The frozen
prelude is rooted independently; later REPL shadowing cannot change existing closures.

### Collector and allocator

A precise nonmoving mark/sweep collector traces registered roots, object edges, code,
modules, and suspended continuations. Marking uses an intrusive worklist without C
recursion or scratch allocation. A one-bit epoch changes each collection; survivors need
no separate mark reset. Sweep releases unreachable allocations and kind-specific owned
storage. Every language-heap allocation may collect before copying its inputs.

C references needed across a safepoint must be rooted, including owners of interior byte
or Value views. Root registrations may be removed independently; their storage must
remain at stable addresses while registered. Only initialized slots are traced. Stress
mode collects at every allocation to expose missing roots.

Use checked sizes and libc allocation. Syntax blocks share code lifetime; frame reuse is
bounded. Buffers and OS bookkeeping have explicit C owners. There is no generic
allocator framework or GC finalization of OS resources.

Trace/sweep work is `O(roots + live objects + live edges + allocated objects)`,
excluding owned-storage disposal. Full collection has no pause bound. The growth
threshold is based on retained bytes; evaluator quanta do not bound GC or native-call
latency.

### Materialization and graph checks

Collectors and sorting fill rooted private builders directly. Capacity stays separate
from the traced prefix. A full builder can become the result; a partial builder is
copied to exact storage before publication. Geometric growth bounds slot-copy work, not
graph charging or GC work.

Retained-byte budgets count distinct reachable backing objects, including captured code
and shared List storage. A traced private set keeps charged objects alive across
incremental calls. Nested materializations have separate budgets; reject excess before
another callback or publication. The budget includes its own storage and is discarded
after error.

Stream escape checks walk all reachable edges, including recursive cells and stage
policies. They reuse an intrusive queue with a separate temporary visit bit, retaining
the complete discovery chain to clear it on success or early rejection. No allocation,
collection, callback, or suspension occurs during this `O(V + E)` walk. Persistent
caches would be invalidated by private builder changes and are not used.

## Resource continuations

`native/stream.c` owns sources, transforms, sinks, and cleanup scopes. Tokens use
non-reused identities; transfer claims the old token and returns a new one. Each edge
retains at most one language item. Byte queues and incomplete text records have separate
bounds defined in [execution](execution.md#consumption-and-materialization).

A complete line within one Bytes chunk is validated and copied directly to its String.
Incomplete line fragments use staging storage. The producer item remains rooted
until the result allocation and copy finish; CRLF handling and raw-byte limits are the
same on both paths.

Consumers propagate demand iteratively. Language callbacks use ordinary evaluator frames
and return callback events; nested consumers have explicit sink frames. Closing releases
dependencies immediately. A doubly linked scope list removes nodes in constant time even
when unrelated chains are interleaved.

The evaluator emits cleanup events at failed `attempt` checkpoints and top-level
statement boundaries. The library closes affected nodes in reverse dependency order and
awaits owned reaping before resuming the rooted result. Unwind visits newer nodes once
and separately follows dependencies transferred from before the checkpoint. Unrelated
earlier resources remain valid. OS progress still determines reaping latency.

`session/evaluation.c` owns the active evaluation and suspended contexts. A saved
context retains continuations, roots, stream scope, pending native operation, and module
load state. Only foreground resumption runs its callbacks. Existing launch snapshots and
lexical captures survive; new session reads observe current cwd/environment. Successful
resumed declarations publish against current bindings, checking nominal conflicts
atomically.

Cleanup is idempotent. Invalidate descriptors on close/transfer and save diagnostics
before releasing borrowed storage. Fatal allocation failure uses a non-allocating
best-effort cleanup path and is not converted to Result.

## Expression editor

The planned editor separates input decoding, edit/undo state, syntax integration,
layout, and rendering. The session owns terminal reads/writes, dimensions, signals, and
helper processes. The editor receives typed events and returns edits, submission
outcomes, and output operations; it has no private wait loop.

Use a growable UTF-8 gap buffer with a byte cursor snapped to extended grapheme
boundaries. Insertion may join adjacent clusters; recompute from a valid segmentation
checkpoint rather than a fixed neighboring byte count. Flatten at most once per revision
for parsing. Bounded undo transactions retain before/after cursor positions; paste and
completion each form one transaction.

Incremental decoding recognizes supported UTF-8, CSI/SS3/ESC keys, and bracketed paste.
Deadlines resolve Escape ambiguity in the shared loop. Unknown/oversized CSI sequences
are drained through their final byte; OSC/DCS/SOS/PM/APC strings are discarded through
ST, or BEL for OSC. Neither timeout nor invalid input may reinterpret discarded bytes as
submitted code. [Interaction](interaction.md) owns keys, paste rules, and limits.

Layout shares text metrics with diagnostics and tables. A viewport preserves the full
entry while showing a bounded region. Reserve the last terminal column, place wraps
explicitly, and never split a grapheme. Render an over-wide cluster as a placeholder
while retaining its source bytes. Fall back when dimensions are unusable.

Rendering queues complete operations and preserves unwritten suffixes across partial
writes. Coalesce future frames without dropping half an emitted escape sequence.
Coordinated notifications invalidate and redraw the edit area; Ctrl-L recovers after
uncontrolled background output. The renderer is not a terminal emulator.

## Process launch and I/O

One `fork`/`execve` backend implements the launch gate and process-group contract.
Prepare argv/envp, cwd, descriptor actions, and bookkeeping before fork. Pipes use
close-on-exec flags; the single-threaded supervisor and descriptor-free signal handlers
prevent a concurrent fork during setup. A future `posix_spawn` path needs measured
benefit while preserving this contract.

1. Allocate control channels and block job-state signals during registration.
2. Fork stages. Children perform only async-signal-safe setup and wait at the gate;
   the parent alone registers their process group.
3. Complete group registration and any foreground terminal handoff, then grant one
   permit per child. EOF without a permit aborts before exec.
4. Restore the parent's signal mask and service launch channels through the event loop.
   A child writes a fixed-size setup/exec error and exits on failure. Channel EOF alone
   proves neither successful exec nor termination.
5. On partial failure, close gates, cancel, restore the terminal, and reap every child.
   A child whose group registration failed retains its separately addressable PID.
   Earlier file creation/truncation is not rolled back.

The gate prevents normal early leader exit before later stages join. External signals
can still interrupt launch and must follow the same cleanup path. Group signals require
an unreaped registered member, never a historical process-group ID.

Children reset the shell's changed dispositions and signal mask before exec. Post-fork
code does not allocate, log, evaluate, or enter the editor. Closed standard descriptors,
`dup2(fd, fd)`, and aliasing need explicit handling. PATH and executable conventions
belong to [platform](platform.md#unix-processes-and-executables).

Nonblocking pumps service stdin and stdout/stderr concurrently with bounded queues and
fair per-channel work. Set O_NONBLOCK only on shell-owned open file descriptions:
[duplication shares status
flags](https://pubs.opengroup.org/onlinepubs/9799919799/functions/dup.html). Partial
I/O, EINTR, and EOF are normal; EOF still requires checked child completion. Inherited
destinations and filesystem calls can block. Stop/cancellation events and escalation
deadlines progress during launch and transport as well as evaluation.

## Supervisor and terminal ownership

Signal handlers preserve errno, set `volatile sig_atomic_t` flags, and write a
best-effort wakeup byte. Full pipes do not lose pending flags. Ordinary code blocks
managed signals while exchanging flags and drains `waitpid` with nonblocking,
stop/continue observations. Crash handling remains with the runtime/sanitizers.

The session owns the controlling terminal, original state, shell modes, and saved job
modes. It waits for foreground membership rather than stealing the terminal.
Terminal-owning `run`/`fg` hands off the foreground group. Data jobs keep the shell
foreground and receive forwarded interrupt/stop events. The planned raw editor borrows
the terminal only during input and restores modes before handoff or suspension.

One loop services jobs, owned I/O, evaluator steps, and monotonic deadlines. Rendering
backpressure must not stop supervision. This is cooperative scheduling, not hard
real-time execution: filesystem calls, inherited writes, collection, and kernel
termination can delay progress.

Completion workers will exec a bounded internal worker entry before directory access.
Permit at most one unreaped worker, including one being cancelled. Expiry invalidates
the result immediately; only wait results establish reaping. Editing never waits inside
the worker's filesystem operation.

## Optimization policy

Measure release workloads separately from sanitizers and external-program costs. Prefer
fewer allocations and edges, bounded live state, contiguous traversal, and immutable
metadata reuse. Object sizes and elapsed time do not establish cache misses, allocator
traffic, or pause bounds. [Testing](testing.md#performance-evidence) owns measurement
practice; [status](status.md#performance-evidence) records evidence.

### Graph equality

Equality validates both complete inputs before identity or mismatch shortcuts, so
unsupported leaves cannot be hidden by field order or sharing. A bounded tree walk
handles small values. Exhaustion restarts with graph validation; it is not a language
error. Validation stores subtree height to enforce depth limits even when a shared node
is reached along a longer path. Active entries reject cyclic data.

Comparison reuses the stable validation table for union by rank and path halving. A
union records child-comparison obligations, not unconditional success. Transitivity
avoids a Cartesian product of equivalent DAG nodes. Disjoint-set work is amortized `O(U
alpha(V))` for `U` operations and `V` aggregates; hashing, edges, and byte comparisons
have separate costs. Expected hash lookup is not an adversarial bound. Indexed Records
zip sorted keys; small Records use bounded linear lookup.

Scratch lives only for the comparison and never crosses GC or callbacks. This follows
bounded pre-check and equivalence ideas from [Adams and
Dybvig](references.md#runtime-memory-design), with Rill's stricter comparability and
cycle semantics.

### Collection and allocation

Retain the nonmoving collector and libc until profiles justify different ownership
contracts. Moving GC must update interior pointers as well as roots. Generational or
incremental GC needs barriers for cells, builders, stage metadata, and budgets;
immutable public values do not remove those writes. Reference counting still needs cycle
handling. Slabs/regions need reclamation and fragmentation policies.

Exact captures, dense prepared metadata, and relocated Record indexes improve storage
without changing allocator contracts. Persistent trees and ropes trade contiguous
traversal for indirection and different costs across versions; introduce them only for
measured workloads.

Layout changes must preserve alignment and improve measured workloads, not only
reduce `sizeof`. Keep ownership and reclamation unchanged unless their replacement
has independent evidence.

[Chez Scheme's generations and relocation](references.md#runtime-memory-design) and
[page-local allocator design](references.md#runtime-memory-design) are useful comparison
points, not drop-in changes. Prioritize removal of unnecessary allocations and repeated
traversals before changing root, barrier, or reclamation contracts. No cache-miss or RSS
improvement follows from the current size and timing measurements alone.

### Preparation and immutable sharing

The [code-owned layouts](#data-and-code-ownership) move repeated name and literal work
into preparation while each closure resolves its own values. For a chain of `d`
functions around `n` nodes with one free name, nested summaries take `O(n + d)` visits
rather than `O(d n)`. This is not a bound for all capture analysis: local-name searches
and wide layouts still cost work.

Constant preparation trades sorting and cold-code work for cheaper repeated reads and
less repetitive storage. Distinct-literal workloads also matter. Preparation runs
outside evaluator quanta; do not cache expressions whose errors, effects, or nominal
identity are observable. Local binding searches, free-name deduplication, module
identities, and environment names still have linear scans; some large-pattern operations
are quadratic. Profile wide scopes before adding symbol tables or persistent
environment trees.

### Bytecode decision

The current evaluator is a prepared AST machine. No bytecode speedup has been measured.
A future replacement should compare small register and stack VMs on identical Rill
workloads, initially using ordinary C dispatch and code-owned instruction, constant, and
source tables. Wider instructions, register liveness, and dispatch count all matter; [VM
references](references.md#bytecode-and-program-analysis) do not establish a universal
winner.

Keep the replacement inside `runtime` and preserve its event protocol. Suspended
contexts must retain execution position, roots, and resource checkpoints. Register reuse
must clear dead references or supply verified liveness maps. Growing storage must rebind
roots and invalidate borrowed views safely.

Do not fuse curried applications, reorder callbacks, fold errors in unexecuted branches,
or remove resource boundaries. Acceptance requires differential semantics, stress GC,
allocation failures, stop/resume, and measurements of startup, code size, retained heap,
and steady-state execution. No serialized bytecode format is promised.
