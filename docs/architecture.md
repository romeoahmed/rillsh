# Architecture

This document defines the implementation strategy and ownership model for the first
release. Behavior is specified in [language](language.md), [execution](execution.md),
[interaction](interaction.md), and [platform](platform.md). The
[implementation plan](implementation-plan.md) maps components to delivery stages;
[current status](status.md) records what is implemented. Representations may evolve while
preserving these contracts.

## Design principles

Keep a small semantic core and express reusable composition in the standard library.
Functions are ordinary unary values; effects remain explicit operations. Immutable
language data coexists with mutable bookkeeping owned by the runtime and supervisor.
Process bytes, structured values, and display text have explicit conversion boundaries.

Parsing and presentation never execute user code. Evaluation preserves effect order;
proper tail calls and streaming bound live state where the semantics permit. OS cleanup
is explicit and independent of garbage collection.

The first release excludes POSIX-shell syntax compatibility, implicit expansion,
mutable bindings, classes, macros, static typing, native plugins, a package manager,
JIT compilation, Windows support, and configurable editor modes.

## Cohesive components

The session coordinates `syntax`, `runtime`, `exec`, `platform`, `editor`, `text`, and
`library`. These internal components define responsibility and ownership. Introduce
interfaces where ownership, reuse, or independent testing requires them; no public C ABI
is provided.

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

Retain owned UTF-8 source, stable SourceIds, and byte-based spans. Derive line and
display coordinates only for presentation. NUL in source is invalid; NUL produced by a
valid string escape is data until an OS boundary rejects it.

A compact recursive-descent/Pratt parser suits the fixed grammar and explicit command
mode. Patterns have their own nodes. Share token kinds, spelling, precedence, and syntax
metadata between parsing, highlighting, and help; do not maintain parallel lexers for
presentation. Parsing has a 256-construct nesting limit and never executes code.
Resolution establishes lexical slots and constructor paths before lowering.

Lower multi-parameter functions to unary functions, pipelines to ordered bindings and
applications, and ADT construction to shared primitives. User functions, native
functions, callbacks, and constructors use one application protocol. Native functions
may return a value, request a language call, yield for I/O, raise an error, or propagate
cancellation. A native higher-order function cannot hide a recursive C evaluator.

Evaluation is a loop over explicit continuation frames for calls, bindings,
conditionals, matching, sequencing, native resumption, and error/resource boundaries.
Tail calls replace the active expression/environment and reuse the caller continuation.
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
carry allocation size, mark state, and trace metadata. Trace with an explicit worklist.

Lists use immutable backing arrays with offset/length views, so recursive suffix
patterns do not copy every tail. A small retained slice can keep a large backing array;
account for that storage and compact only at a measured materialization boundary.
Anonymous records use a shape with unique names, a key index, and values in presentation
order. Shape/intern caches must be collectible or bounded.

Nominal products and sum variants share a descriptor, constructor index, and field
array. Fieldless variants are descriptor-owned singletons. Constructor functions carry
descriptor metadata and implement ordinary unary application. No C type or bespoke
evaluator branch is generated for each user ADT.

Closures retain code and exactly the resolved captures specified by the language.
Recursive function groups allocate slots before installing closures. Native partial
applications retain their native identifier and bound arguments. Code objects own
source/constants for as long as reachable closures need them; entry-scoped scratch
storage cannot own code used by later entries.

REPL publication updates the visible binding map. Shadowed bindings survive only when
retained by closures or other reachable values; an ever-growing chain of entry scopes
must not keep every replaced value alive. History owns source text, not evaluation
environments.

A precise non-moving mark-and-sweep collector traces values, code, closures, modules,
continuations, and references held by live execution contexts. Every allocating
operation is a potential safepoint. C references needed afterward must be in explicit
native root frames. Test collection at every safepoint. Parsing/lowering scratch may use
arenas; long-lived stream items and tail-call environments must be collectible before
the top-level command ends.

Use the system allocator with checked sizes. Editor buffers and OS bookkeeping have
explicit C lifetimes; they are not language heap objects and need no GC integration
unless they retain a rooted language reference. Avoid a general allocator abstraction
until measurement or fault-injection boundaries justify one.

The initial heap stores each object's value array and terminated immutable bytes in
one allocation; its `RillBytes` view cannot grow or be freed separately. Continuation
frames likewise carry their operand array in a flexible member. Borrowed byte spans are
shared across text/runtime/exec interfaces, while `RillBuffer` alone owns growable text.

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

The editor is project code with six narrow responsibilities:

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
descriptor-free signal handlers prevent a concurrent fork during this setup; no
platform feature probe or duplicate backend is needed.

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

Prioritize bounded streams, efficient lexical slots, shared immutable data, in-process
transforms, a gap buffer, and derived metadata reused within one revision. Maintain
incremental unique-object accounting for materialization budgets instead of rescanning
all collected data after every item. Stable sort may use the C library by decorating
keys with original indices, preserving equal-key order without another sorting engine.

Measure before adding a custom allocator, persistent-tree collection, thread pool,
bytecode, specialized platform fast path, or LTO requirement. Compare release builds
separately from sanitizer runs; separate external-program/container time from shell
cost. Tail recursion and streams must exhibit bounded live heap where semantics allow.
