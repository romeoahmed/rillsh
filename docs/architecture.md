# Architecture

This document defines the implementation strategy and ownership model for the first
release. Behavior is specified in [language](language.md), [execution](execution.md),
[interaction](interaction.md), and [platform](platform.md). The [implementation
plan](implementation-plan.md) maps components to delivery stages; [current
status](status.md) records what is implemented. Representations may evolve while
preserving these contracts.

## Design principles

Keep a small semantic core and express reusable composition in the standard library.
Functions are ordinary unary values; effects remain explicit operations. Immutable
language data coexists with mutable bookkeeping owned by the runtime and supervisor.
Process bytes, structured values, and display text have explicit conversion boundaries.

Parsing and presentation never execute user code. Evaluation preserves effect order;
proper tail calls and streaming bound live state where the semantics permit. OS cleanup
is explicit and independent of garbage collection.

The first release excludes POSIX-shell syntax compatibility, implicit expansion, mutable
bindings, classes, macros, static typing, native plugins, a package manager, JIT
compilation, Windows support, and configurable editor modes.

## Cohesive components

The session coordinates `syntax`, `runtime`, `exec`, `platform`, `text`, and `library`;
stage 4 adds `editor`. These internal components define responsibility and ownership.
Introduce interfaces where ownership, reuse, or independent testing requires them; no
public C ABI is provided.

The editor consumes parser services and materialized metadata without invoking the
evaluator; the parser has no editor dependency. The process supervisor receives
validated launch data without depending on ASTs or language Values. Components return
structured diagnostics for rendering at the session boundary. Language and editor-core
tests need neither a terminal nor yyjson.

One thread owns mutable runtime/session state. One event loop services jobs, editor
events, I/O, timers, and evaluator safepoints. Native state machines can yield to that
loop without a recursive evaluator or nested wait loop. Completion workers are
supervised helper processes; no user evaluator runs in them.

## Syntax and evaluation

Retain owned UTF-8 source, logical source identities, and byte offsets. Derive line and
display coordinates only for presentation. NUL in source is invalid; NUL produced by a
valid string escape is data until an OS boundary rejects it.

A compact recursive-descent/Pratt parser suits the fixed grammar and explicit command
mode. Patterns have their own nodes. Share token kinds, spelling, precedence, and syntax
metadata between parsing, highlighting, and help; do not maintain parallel lexers for
presentation. Parsing has a 256-construct nesting limit and never executes code. Nodes
occupy stable, typed blocks growing from 8 to at most 128 nodes per block; individual
decoded-text buffers retain their own ownership. Clearing a parse frees buffers and
blocks iteratively, including incomplete and failed parses. Block slack is included in
code's retained-byte accounting. Code preparation computes each function's free-name
layout once and records duplicate pattern names for later diagnosis. Matching does not
rebuild a name-validation list on every call; subject/argument effects still precede
pattern errors. Preparation assigns String literals and Record keys dense slots in the
code object's traced Value array. These annotations reuse node padding and kind-specific
payload storage; the syntax component depends on no runtime type. Closure creation
resolves those names to fixed captured bindings. Local environments and constructor
paths remain lexical; no evaluation consults a later REPL scope.

Lower multi-parameter functions to unary functions, pipelines to ordered bindings and
applications, and ADT construction to shared primitives. User functions, native
functions, callbacks, and constructors use one application protocol. Native functions
may return a value, request a language call, yield for I/O, raise an error, or propagate
cancellation. A native higher-order function cannot hide a recursive C evaluator.

Evaluation is a loop over explicit continuation frames for calls, bindings,
conditionals, matching, sequencing, native resumption, and error/resource boundaries.
Tail calls replace the active expression/environment and reuse the caller continuation.
Atomic operand literals and name references write directly into the parent's rooted
operand array. They cannot suspend or execute user code, so they need no separate frame;
String literals read a prepared constant slot without allocating; unknown names retain
the operand's source offset. Other expressions keep the ordinary continuation protocol.
Abandoned arguments, pattern bindings, and environments lose roots promptly. Keep
backtraces proportional to live continuations; a tail-call summary is bounded. The
non-tail continuation limit is 65,536 frames, reported as LimitExceeded.

Use a lowered-AST machine initially. Bytecode, specialization, and allocation elision
require measured benefit and preservation of intermediate application/effect order.
Standard-library source contains ordinary reusable composition; C primitives supply OS
access, resource ownership, and operations that need efficient representation access.

## Values and memory

Represent values with a C tagged union: immediate Int/Float/Bool/Null/Unit and pointers
to typed heap objects. Do not introduce NaN boxing or pointer-tag assumptions. Objects
carry allocation size, allocation kind, mark state, and trace metadata. A tagged union
shares mutually exclusive byte/name views, code/budget/slice state, closure layouts,
Record indexes, and stage policies. Only stages have a traced metadata Value; other
union members must never be interpreted as edges. The header occupies 64 bytes on the
validated AArch64 ABIs; this is a measured layout, not a portable ABI promise. Non-text
objects do not reserve a byte terminator. Collector links, kind/mark state, sizes, and
edge counts precede the kind-specific payload. Values and immutable byte payloads follow
the header. Compile-time assertions check flexible-array placement and interior
pointer-index alignment; each index uses the size of its actual pointer type.

Known-size aggregates are built directly in private heap payloads initialized to Unit.
Root the owner before allocating children, then finish Record indexes before publishing
the value. This avoids temporary C arrays and a second copy. Borrowed inputs still need
roots across the initial allocation, and any failure must discard the partial result.

Lists use immutable backing arrays with offset/length views, so recursive suffix
patterns do not copy every tail. A small retained slice can keep a large backing array;
account for that storage and compact only at a measured materialization boundary. A
whole-range slice reuses its input; an empty slice has no backing-array edge. Discarded
rest patterns construct neither a slice nor a remainder record. Anonymous records store
unique, length-aware String keys and values in presentation order. Records with at least
16 fields reserve one interior key pointer per field in the same allocation.
Finalization sorts this index with libc `qsort`; lookup uses binary search, including
embedded-NUL keys. Small records use linear lookup and no index storage. The index adds
no GC edges or separate lifetime, and the object header does not grow. Builders finalize
after filling or shrinking their arrays; immutable updates preserve keys and change only
values in a new record. Dynamic construction validates adjacent sorted keys rather than
comparing every pair. Libc may use temporary sorting storage; no allocation-free sorting
guarantee is made.

Nominal products and sum variants use unique constructor descriptors and immutable
payload records. Fieldless variants are singleton values. Constructor functions carry
descriptor metadata and implement ordinary unary application. No C type or bespoke
evaluator branch is generated for each user ADT.

Closures contain one code edge followed by a contiguous array of exactly the resolved
captures specified by the language. They also borrow a capture layout from that code;
there is no separately allocated environment node or copied name per capture. Layouts
with at least eight captures sort names once for binary lookup; smaller layouts use
linear lookup. The closure itself terminates a local environment chain. Recursive
function groups allocate traced binding cells before installing closures.
Standard-library wrappers express native currying through ordinary closures. Code
objects consume and reset parse results, including on failure, instead of reparsing
source. They own source, syntax blocks, fixed capture-analysis tables, and a String
constant pool until the code becomes unreachable. The pool is completed under an
explicit root before execution; it also serves literal patterns. Lowered String nodes
release their redundant decoded buffers with matching accounting updates. Record keys
retain decoded text for pattern field lookup. Constants do not point back to code, so an
escaped String retains only its own allocation. The pool neither interns across code
owners nor caches closures, nominal descriptors, or mutable builders. Analysis stores
names, never captured values; instances still resolve their own bindings, including
recursive cells and nominal constructor paths. Analysis diagnostics are deferred to
closure creation, so dead branches do not acquire new runtime errors. All analysis
storage is charged before code publication; it cannot silently grow after a
materialization budget has charged that code. The module wrapper is part of the code
owner. Entry-scoped scratch cannot own code used by later entries. A module completion
event carries a fully constructed export Record; construction failure returns an error
before the session can cache it.

REPL publication updates the visible binding map. Shadowed bindings survive only when
retained by closures or other reachable values; an ever-growing chain of entry scopes
must not keep every replaced value alive. Entries without new bindings reuse the
committed environment. An entry with bindings sorts candidate names with explicit
recency tie-breaking, keeps the newest value of each name, and constructs a single
immutable snapshot. Values, name pointers, and terminated names share its allocation.
Lookup is binary for at least eight bindings and linear below that threshold. Sorting
uses libc; the remaining work is linear in candidate count and copied name bytes.
Scratch borrows the rooted input until the finished snapshot can be published
atomically. The snapshot retains no preceding environment; an explicitly frozen prelude
remains independently rooted. Local scope chains still enforce same-scope duplicate
checks. Single-name function parameters need no empty scope object or pattern-name
scratch list. Compound patterns use their prepared duplicate-name flag and still perform
all shape, nominal-identity, and value checks at matching time. Tail replacement also
drops the previous result root. These changes preserve lexical shadowing, effect order,
and transactional publication. History owns source text, not evaluation environments.

A precise non-moving mark-and-sweep collector traces values, code, closures, modules,
continuations, and references held by live execution contexts. Marking is iterative and
uses an intrusive worklist without allocating scratch storage. Leaf objects are marked
without entering the worklist. A one-bit heap epoch flips at each collection; fresh
objects carry the heap epoch at allocation and become unmarked when it next flips.
Tracing records the new epoch. Sweeping need not clear survivors' mark bits. This
removes one store per live object, not a graph traversal or a pause bound. Sweeping
releases owned syntax and budget storage by allocation kind. The next threshold is twice
retained bytes, with a 64 KiB floor and saturating arithmetic; stress mode collects
before every object allocation. Every language-heap allocation is a potential safepoint.
C references needed afterward must be in explicit native root frames. Test collection at
every safepoint. Syntax blocks follow code lifetime; long-lived stream items and
tail-call environments must be collectible before the top-level command ends.

Use the system allocator with checked sizes. Editor buffers and OS bookkeeping have
explicit C lifetimes; they are not language heap objects and need no GC integration
unless they retain a rooted language reference. Avoid a general allocator abstraction
until measurement or fault-injection boundaries justify one. The evaluator caches at
most 32 inactive frames of 16 Value slots each. Larger frames return directly to libc;
all cached frames are freed with the evaluator. Reuse avoids allocator traffic on
ordinary calls without retaining an unbounded stack high-water mark. Only Lists,
Records, enums, plans, and stages size frames by retained operand count; sequential
blocks, branches, and matches do not reserve storage proportional to syntax width.
Unused slots are neither read nor registered as roots; only initialized live slots
participate in GC.

Borrowed `RillBytes` views never grow or own storage; `RillBuffer` owns growable bytes.
Continuation frames carry their operands in a flexible array. Sequence materialization
and sorting fill rooted private builders directly, avoiding an intermediate Value array.
These writes are construction, not language-visible mutation.

Materialization budgets retain a private set of charged backing objects. Incremental
charging counts shared graphs once, including captured code, and rejects excess before
the next callback. The set is traced and its storage is accounted by the heap; it owns
no OS resources. Nested materializations use independent budgets.

The ownership map includes the planned stream, codec, and editor components:

| Owner                | Resources                                                              |
| -------------------- | ---------------------------------------------------------------------- |
| Session              | Terminal, signal channel, history, background jobs, active editor      |
| Execution context    | Continuations, launch snapshots, resource scopes                       |
| Job                  | Child identities, process group, launch channels, captured descriptors |
| Stream control block | Producer, queues, generation, transferred upstream ownership           |
| Editor state         | Text, undo records, revision, completion view, layout caches           |
| Codec invocation     | yyjson document and temporary conversion storage                       |

Codec invocations own their yyjson documents and copy decoded values into the runtime
heap. They never parse in-situ over immutable String/Bytes storage or retain document
pointers after freeing the document. Use an explicit conversion worklist and validate
depth at conversion; the input-byte limit applies before parsing. A codec byte limit is
not a promise that parser and decoded-heap overhead fit in that many bytes.

Cleanup is idempotent and follows dependency order. Invalidate an owned FD immediately
on close/transfer and snapshot errno before cleanup. Fatal allocation failure uses a
preallocated diagnostic and a non-allocating best-effort cleanup path; it is not a
recoverable Result. Ordinary failures use explicit status values and cleanup blocks.

## Expression editor

Stage 4 adds an editor implemented in project code with six narrow responsibilities:

| Part               | Input and output                                              | Excludes                         |
| ------------------ | ------------------------------------------------------------- | -------------------------------- |
| Terminal adapter   | Device reads/writes, dimensions, saved modes                  | Grammar, editing decisions       |
| Input decoder      | Byte chunks and deadlines to typed input events               | Evaluation and filesystem access |
| Edit state         | Events to text/cursor/undo changes and requested actions      | Syscalls and terminal escapes    |
| Syntax integration | Buffer revision to parse status, indentation, highlight spans | Running user code                |
| Layout             | Text, styles, viewport to rows/cells and cursor coordinates   | Terminal I/O                     |
| Renderer           | Previous/next layout to bounded output bytes                  | Parsing or modifying source      |

Use explicit events such as Text, Key, PasteBegin/Chunk/End, Resize, Interrupt, Suspend,
and CompletionReady. The session routes signal/process events and calls editor steps;
the editor neither installs handlers nor owns a private wait loop. Submission produces
owned source or a distinct Cancel/EOF/Error outcome.

Start with a growable UTF-8 gap buffer and byte-offset cursor. Edits and navigation snap
to extended grapheme boundaries. Insertion can merge adjacent clusters; update the
boundary index and cursor after the change. Segmentation is supplied by `text` using
generated Unicode properties. Recompute from a valid segmentation checkpoint; a fixed
number of neighboring bytes is not a correct Unicode context bound.

Undo records are bounded insert/delete transactions with before/after cursor positions.
Coalesce continuous typing; paste and completion are atomic edits. Revisions increase on
every text change. Flatten at most once per revision when a contiguous parser view is
needed; cache that view and derived spans. The common mutation path should not copy the
entire entry for every input byte.

Input decoding is incremental across arbitrary reads. The bounded state machine
recognizes UTF-8, supported CSI/SS3/ESC-prefixed keys, and bracketed paste. Escape
ambiguity uses a deadline in the shared loop, not a blocking read. Unknown control
sequences are discarded without turning their payload into submitted code. Retain only
bounded key-decoding state; drain an unknown or oversized CSI through its final byte.
Recognize control-string boundaries (OSC/DCS/SOS/PM/APC) only to discard through ST, or
BEL for OSC, without retaining their payload. Deadlines must not reinterpret a discarded
payload as keys. Pending UTF-8 is not guessed from the user's locale. Paste validation
and rejection follow interaction; a rejected transaction is drained without interpreting
shortcuts.

Layout shares `text` metrics with diagnostics and tables. Use a viewport for entries
larger than the terminal; retain the full buffer independently. Start with correct
redrawing of the owned edit area, then optimize changed rows after measurements. Avoid
dependence on implicit right-margin wrapping: reserve the final terminal column, place
wrapped rows explicitly, and fall back when dimensions are too small. Never split a
grapheme to satisfy a row boundary. A cluster wider than the viewport uses a visible
placeholder while retaining its source bytes.

Queue complete rendering operations, handle partial writes, and keep their unwritten
suffix intact. Coalesce obsolete future frames, never discard half of an emitted escape
sequence. Shell-generated notifications invalidate layout, print through the same output
owner, and restore the editing view. After uncontrolled background output, Ctrl-L
establishes a fresh anchor and redraws. Do not implement a terminal emulator or attempt
to track arbitrary escape sequences emitted by external programs.

Use generated Unicode properties, shared syntax/builtin metadata, and small static key
tables. Keep editor state independent of terminal I/O so input and rendering can be
tested without a live terminal.

## Process launch and I/O

Use one shared `fork`/`execve` backend to implement the launch gate and foreground
process-group contract. Consider `posix_spawn` only as a measured optimization that
preserves those semantics. PATH search and executable conventions are specified in
[platform](platform.md). Completion helpers exec an internal worker entry point before
allocating or performing directory queries; they obey the same post-fork discipline.

Prepare argv/envp, cwd, descriptor topology, redirection paths, and bookkeeping before
fork. Use close-on-exec channels and validated descriptor actions. Keep signal handlers
from opening descriptors or launching children. One platform helper creates pipes with
the required flags using `pipe` and `fcntl`. The single-threaded supervisor and
descriptor-free signal handlers prevent a concurrent fork during this setup; no platform
feature probe or duplicate backend is needed.

1. Allocate bookkeeping/control channels; block job-state signals during registration.
2. Fork stages; the parent establishes each child's intended process group.
3. Children perform audited setup only: cwd, descriptors, signal state, launch gate.
   On failure, write a fixed-size setup/exec error record to the close-on-exec channel
   and `_exit`; successful setup proceeds through the gate to exec.
4. Hold prepared children at the gate until the group is registered and any foreground
   terminal handoff is complete. The gate prevents normal early exit of the leader
   before later stages join; an externally killed child still follows failure cleanup.
5. Grant one gate permit per child only after successful registration and handoff;
   EOF without a permit aborts the child before exec. The gate makes the parent the
   sole owner of group registration, avoiding concurrent parent/child `setpgid` calls.
   Restore the parent's signal mask and service channels through the
   event loop. EOF on an error channel means no more error records, not proof of
   successful exec or process completion; a pre-exec signal can also close it.
6. On partial failure, close gates without permits, cancel owned children, restore the
   terminal, close descriptors, and reap every child. Prior file truncation is not
   rolled back. Retain a separately addressable PID for a child whose group registration failed;
   even a stopped unregistered child must be continued/cancelled and reaped. Group
   signals require an unreaped registered member, never just a historical group ID.

Children reset the dispositions changed by the shell, including ignored SIGPIPE and
job-control signals, and unblock the intended mask. Post-fork code does not call the
allocator, evaluator, editor, or logger. Audit it against async-signal-safe interfaces.
Handle closed standard descriptors, `dup2(fd, fd)`, and descriptor aliasing explicitly.

Nonblocking pumps service owned stdin pipes and stdout/stderr capture pipes
concurrently. Apply O_NONBLOCK only to shell-owned open file descriptions. Duplicating
an inherited FD [does not isolate its status
flags](https://pubs.opengroup.org/onlinepubs/9799919799/functions/dup.html); never
change caller-owned flags as a shortcut. The editor opens its own terminal description.
Inherited destinations can still block on writes, just as ordinary filesystem calls can
block.

Partial reads/writes and EINTR are normal; EOF still requires stage completion. Bound
queues, apply backpressure, and avoid draining one FD to starvation of others. Stop,
cancellation, and deadline events progress during launch as well as execution. Use POSIX
`poll` and `waitpid` with WNOHANG/WUNTRACED/WCONTINUED. A stream poll produces Item,
Pending, End, or Error; Pending registers interests with this loop.

## Supervisor and terminal ownership

Signal handlers save/restore errno, set `volatile sig_atomic_t` flags, and write a
best-effort byte to a nonblocking self-pipe. They do not allocate, invoke C callbacks
with arbitrary signatures, or enter the editor. Drain flags and repeat `waitpid` in
ordinary execution context; full notification pipes must not lose pending state. The
flag exchange blocks relevant signals to avoid clearing a concurrently set flag. Leave
crash handlers to the runtime/sanitizers. Handle normal termination through owned-job
cleanup rather than swallowing default termination indefinitely.

The session owns the controlling terminal, original flags/termios, shell modes, and
stopped-job modes. The editor borrows it only during input. Check foreground membership
before acquiring interactive control; a background shell stops with SIGTTIN instead of
stealing the terminal. Raw editing disables device-generated control-key signals, so the
decoder maps those keys to host actions outside paste. Before running a program or
suspending, drain/finish editor mode changes and restore the appropriate attributes. Raw
mode never leaks into external commands.

Terminal-owning `run`/`fg` transfers the foreground process group.
Capture/stream/through keep the shell foreground and receive forwarded interrupt/stop
events. Pure evaluation checks those events at safepoints. Session effects follow
ordinary language call order; each new launch snapshots its environment independently of
existing jobs.

Reap completed jobs while input is idle. Service owned I/O and evaluator work in bounded
quanta using monotonic deadlines. A full editor output queue pauses rendering, not
supervision. This is cooperative scheduling, not hard real-time execution: a blocking
native filesystem call or inherited-destination write can delay progress until it
returns or the OS interrupts it. Do not use restart-on-signal indiscriminately.

Completion isolates directory access in a worker. Permit at most one unreaped completion
worker, including a worker being cancelled. Expiry removes its result from consideration
immediately; cleanup continues in the supervisor, and another worker cannot accumulate
behind it. Cancellation deadlines schedule escalation; only wait results establish
reaping.

## Optimization policy

Measure release workloads separately from sanitizer tests and external-program costs.
Prioritize fewer allocations and edges, bounded live state, contiguous traversal, and
reuse of immutable metadata. Tail calls bound live continuations, not total allocation
traffic. Do not infer cache misses or latency bounds from object size or throughput.

### Graph equality

Equality validates both inputs before identity or mismatch shortcuts, so unsupported
leaves cannot be hidden by field order or sharing. A tree walk first tries validation
and comparison within 64 iterations and 64 local frames. Exhaustion restarts with graph
validation; it is internal, not a language error. This bounded prefix avoids memo setup
for small data without expanding a large shared DAG indefinitely.

Graph validation registers each aggregate and stores subtree height, enforcing the
65,536-frame depth limit even when a shared node is reached along a longer path. An
active entry denotes a cycle and returns LimitExceeded. User data is acyclic; recursive
closures are unsupported equality operands regardless of GC support for their cycles.

Comparison reuses the completed table for union by rank and path halving. The table no
longer grows, so interior parent pointers stay stable; height storage becomes rank
storage. A union records child-comparison obligations, not unconditional success. All
scheduled children must agree. Transitivity avoids enumerating every possible pair when
equal DAGs have different sharing. This follows the bounded pre-check and equivalence
ideas in [Adams and Dybvig](references.md#runtime-memory-design), without Chez Scheme's
randomized interleaving or cyclic-data semantics.

For `V` aggregates and `U` union/find operations, disjoint-set work is amortized `O(U
α(V))`. Hashing, graph edges, key lookup, and byte comparisons are separate costs;
expected hash lookup is not an adversarial worst-case guarantee. Two indexed Records zip
their sorted keys and values in linear field work. Small Records use bounded linear
lookup; String comparison still costs its byte length.

Scratch starts with 64 local frames and hash slots and grows with checked sizes. Tables
stay at most half full. No GC or user callback runs while scratch borrows objects.
Scratch is discarded on every outcome and uses space proportional to distinct objects
plus traversal depth, independently of the number of encountered pairs. Do not cache
comparability on objects that a private builder can still change.

### Collection and allocation

Trace and sweep cost `O(roots + live objects + live edges + allocated objects)`,
excluding owned-storage disposal. Compact headers, flat captures, and kind-specific
tracing reduce storage and visits without changing that complexity. A closure with `k`
captures uses one allocation with `k + 1` traced slots; captured objects and recursive
cells keep their own lifetimes. Exact captures matter: a syntactic recursive group need
not be a strongly connected component, and sharing its entire environment can retain
unrelated data.

Keep the nonmoving collector and libc allocation until profiles justify a different
ownership contract. Moving collection must update interior byte/Value pointers as well
as roots. Generational or incremental collection needs barriers for recursive cells,
private builders, stage metadata, and growing budgets. Immutable public data does not
remove those writes. Reference counting still needs cycle handling. Full collection has
no bounded pause guarantee; evaluator quanta do not bound GC or native-call latency.

Typed syntax blocks share code lifetime; arbitrary closure pools tied to entry
completion would defeat bounded live space during tail recursion. A slab or region
allocator also needs reclamation, size/line metadata, and a fragmentation policy.
Persistent trees and ropes trade contiguous traversal for indirection and different
amortized costs across versions. Add these mechanisms only for measured workloads that
need them.

### Remaining costs

Local scope chains, native names, module identities, environment names, and pattern-name
validation still have linear scans. Large scopes, source-record duplicate checks, and
Record-rest matching can do quadratic work. Snapshot compaction does not remove those
costs. Lexical slots, interning, record shapes, and bytecode would change resolution or
ownership boundaries and need supporting profiles.

Code preparation analyzes unexecuted functions and builds constants, including unused
literals. It trades cold-code work and allocation for cheap repeated reads and runs
outside evaluator quanta. Do not extend caching to expressions whose failures, effects,
nominal identity, or capture lifetime are observable. Process supervision is bounded by
job/stage limits; profile its OS work separately from in-process traversal.
