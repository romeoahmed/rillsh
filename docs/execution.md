# Execution

This document defines commands, process jobs, scoped streams, and effectful library
operations. [Language](language.md) owns evaluation order.

## Plans, jobs, reports, and streams

| Object    | Meaning                                         | Lifetime                       |
| --------- | ----------------------------------------------- | ------------------------------ |
| JobPlan   | Immutable external pipeline description         | Ordinary GC-managed value      |
| Job       | Process group, children, descriptors, state     | Explicit session/job ownership |
| JobReport | Immutable completion data for all stages        | Ordinary GC-managed value      |
| Stream    | Single-consumer traversal with possible failure | Explicit execution scope       |

A JobPlan contains evaluated, validated command arguments and can be launched
repeatedly. Each launch creates a distinct Job. A JobReport retains completion data
without granting authority to signal a process. Plans retain execution descriptions, not
unevaluated source.

## Commands and plan construction

```rill
let message = "hello world"
let paths = [path "a.txt", path "b c.txt"]

^printf "%s\n" $message
^cat ...$paths

let subjects = plan {
  ^git log -n 100 --format=%s | ^sort
}
run subjects
```

`plan { ... }` contains exactly one external pipeline, with optional newlines around `|`.
It constructs a plan. A command statement beginning with `^` outside this form lowers to
`run` of that plan and returns Unit on success. Commands are statements, not general
expressions; use `plan` and an execution function inside expressions. The process pipe
`|` is only legal in this command grammar. `|>` cannot be attached directly to an
executing command statement: an explicit stream/capture bridge is required. A leading
process pipe on a following line continues a pipeline only while that source is still
buffered; use a trailing `|` to request continuation before an interactive entry is
submitted.

Command words use these rules:

- The word immediately after `^` identifies the executable. It can be literal or
  a scalar substitution; no alias or language-function fallback is performed.
- An unquoted word is literal text. `*`, `?`, and `~` have no expansion behavior.
  Parentheses and brackets are also literal inside command words; `$(...)` explicitly
  enters an expression.
- Quoted words use the language's string rules and never interpolate internally.
- `$name` and `$(expression)` each contribute exactly one String, Path, or Bytes
  argument. NUL is rejected. Other types require explicit conversion.
- `...$name` and `...$(expression)` require a List of valid scalar arguments and
  contribute one argument per element, including empty arguments. An empty list
  contributes none. Spreading a stream is rejected.
- Adjacent literal, quoted, and substituted fragments without whitespace are
  rejected; use string concatenation inside `$(...)` for a single composed word.
- Space, tab and carriage return separate words. Unquoted `|`, `;`, braces, and
  redirection operators are syntax and must be quoted to be literal. A newline ends
  a complete command. Other Unicode whitespace inside a word remains literal.
- There is no implicit globbing, command substitution, backtick syntax, escape-based
  line continuation, word splitting, or removal of trailing newlines.

Plan arguments and redirection paths evaluate left-to-right during construction,
including across stages. Creating a plan does not launch its described processes or open
its redirection files. Embedded expressions are ordinary strict expressions and may
themselves have effects; plan syntax does not make them pure.

Argument diagnostics use zero-based stage and `argv` indices, with the executable at
index zero. Redirections do not occupy argument indices; each expanded List element
does. Invalid redirection paths are reported separately from argument failures.

`command executable arguments` constructs the same one-stage plan programmatically.
`pipe left right` composes plans and preserves their per-stage policies. There is no AST
re-parsing. `with_cwd path pipeline` and `with_env record pipeline` return new plans with
overrides applied to every stage. Repeated overrides replace the same key; other stage
settings remain intact. Environment overrides overlay the launch snapshot and use the
same name/value validation as `set_env`.

`without_env names pipeline` removes the listed environment names from every stage's
launch environment. Later `with_env` can restore a name; neither operation mutates
session state. `with_stdin path pipeline` adds input redirection to the first stage.
`with_stdout`, `append_stdout`, `with_stderr`, and `append_stderr` take path then plan
and add a redirect to the final stage; `stderr_to_stdout pipeline` duplicates final stderr
to its then-current stdout. Redirects append in application order. Decorate a component
before `pipe` to target an earlier stage. Files open at launch, not plan construction.

Unspecified cwd, environment, and PATH are snapshotted at launch. Explicit cwd overrides
are resolved to an absolute directory at plan construction so later `cd` does not
reinterpret them. Relative command arguments and redirection paths remain relative to
the launch cwd. Unqualified executables are found using that launch's PATH. Reusing a
plan therefore does not promise identical external state or output.

Command statements (`^program ...`) always invoke the standard checked runner. A lexical
binding named `run` affects explicit `run pipeline` calls only; it cannot change the meaning
of command syntax. `execute` retains ordinary foreground I/O and exposes unsuccessful
exit status as data. Setup/exec failures still raise launch errors, and user
cancellation follows the separate cancellation path.

## Redirection

Redirections are `<`, `>`, `>>`, `2>`, `2>>`, and `2>&1`. Other descriptor
redirections and here-documents are rejected. Paths are a single literal/substituted
word. `>` truncates; `>>` appends. File creation uses ordinary permissions filtered by
the process umask.

Pipeline endpoints are installed first; each stage's redirections then apply
left-to-right. Thus `2>&1 > file` and `> file 2>&1` deliberately differ. A file
redirection may replace a pipe endpoint; this is legal and documented rather than
silently rewritten. All unused pipe copies must still be closed, allowing EOF or SIGPIPE
to occur.

Open failures abort launch and clean up all already-created resources. Redirection is
not transactional: a file opened earlier may already have been created or truncated;
redirection does not promise atomic file replacement.

## Execution functions

| Function           | Behavior                                                                                                |
| ------------------ | ------------------------------------------------------------------------------------------------------- |
| `execute pipeline` | Foreground execution with inherited terminal streams; return JobReport without applying exit policy     |
| `run pipeline`     | Foreground execution, inherited terminal streams; check all stage policies; return Unit                 |
| `capture pipeline` | Foreground execution, concurrent stdout/stderr capture; return report and Bytes; nonzero exit is data   |
| `stream pipeline`  | Foreground job source producing stdout Bytes chunks; stderr inherited; completion checked at stream end |
| `start pipeline`   | Start external-only background job; return session Job handle                                           |
| `wait handle`      | Wait for a background job's report; nonzero exit is data; a stopped job raises JobStopped               |
| `check report`     | Reject cancelled or failed reports; otherwise Unit                                                      |
| `fg handle`        | Resume a job/context; return Unit for an external-only checked job or the resumed evaluation's result   |
| `bg handle`        | Resume a stopped external-only job without terminal ownership; return Unit                              |
| `cancel handle`    | Cancel and await cleanup; return Unit; completed jobs are unchanged                                     |
| `jobs ()`          | Immutable snapshots of session jobs                                                                     |

`start`, `stream`, and `through` initiate launch when called. They complete the launch
handshake before returning a handle; setup/exec error records raise a launch error. A
returned handle is not a promise of successful process termination. Transforming a
stream is lazy with respect to pulling items, not with respect to effects already
performed while constructing its source.

`capture` returns a record with `stdout`, `stderr`, and `report` fields. It preserves
bytes exactly, including trailing newlines and NUL. Launch failures and capture resource
limits still raise errors; a command that starts and exits nonzero produces data.
Capture has a default combined output limit of 64 MiB; `process.capture_with options pipeline` can
set a larger explicit limit. Exceeding the limit cancels/reaps the job and raises
LimitExceeded with counts, never returns silently truncated success.

Absent explicit redirection, `capture` and `stream` use `/dev/null` for stdin so a data
source does not unexpectedly read from the user's terminal. `run` and `execute` inherit
stdin. `start` defaults stdin to `/dev/null` and inherits stdout/stderr; background
output may appear while editing. Explicit redirection overrides these defaults.
`capture`, `stream`, and `through` are foreground evaluations but keep the shell in the
terminal foreground group; the supervisor forwards terminal signals to their attached
process groups. An inherited stderr TTY destination is relayed through a bounded
supervisor pipe, so data-job groups do not stop merely because the terminal has TOSTOP
enabled. Non-TTY inherited stderr may be connected directly. Programs requiring direct
access to the controlling terminal must use `run` or `execute`.

`through pipeline byte_stream` is the streaming process sink/source: it feeds the first
stage's stdin and exposes the last stage's stdout while inheriting stderr. It
concurrently pumps input and output and checks completion. Conflicting explicit stdin or
stdout redirection is rejected for `through` instead of discarding the supplied stream.
Normal exit, downstream cutoff, and errors propagate upstream.

```rill
stream plan { ^git log --format=%s }
  |> lines
  |> filter (starts_with "fix:")
  |> take 10
  |> collect

files (path ".")
  |> map { e => {name: display_path e.name, size: e.size} }
  |> collect
  |> to_json
  |> chunks
  |> through plan { ^cat }
  |> write_stdout
```

`to_json` encodes one materialized value to Bytes; `chunks` turns Bytes into a byte
stream. There is no dedicated NDJSON codec. Compose `lines` with `map from_json` for one
JSON value per line; process chunk boundaries never imply document boundaries.

## Job states and completion

A process job moves through Launching, Running, Stopped, Cancelling, and Completed. Its
immutable JobReport contains `id`, `stages`, `completion`, and `failure`.

```rill
enum Termination {Exited {code}, Signaled {signal}}
enum Completion {Finished, Cutoff {reason}, Cancelled {reason}}
```

`stages` is an ordered List of records with `index` (zero-based), `termination`,
`accepted_codes`, and `expected_cutoff` (Bool). `completion` distinguishes normal
completion, intentional consumer cutoff, and cancellation, even if every process
happened to exit zero. Reasons are diagnostic Strings. `failure` is Option of the
selected unacceptable stage index. `check` raises ProcessError for a cancelled report or
a present failure; ordinary report values do not themselves cancel the calling
evaluation.

Default accepted exit codes are `[0]`. `accept_exit codes pipeline` replaces the final
stage's policy with a nonempty List of codes in 0–255. Reports normalize this set to
ascending unique codes. Configure an earlier stage before composition. Reports retain
these policies and actual terminations after expected-cutoff classification; they never
rewrite a signal into exit zero.

Aggregate failure is the rightmost unacceptable stage in pipeline order, after
accounting for expected cutoff. Launch errors are separate from process termination: an
actual exit code 127 must not be confused with failure to execute.

SIGPIPE is expected only when its downstream path completed successfully without an
intervening stage failure, or when the supervisor recorded a successful explicit cutoff
such as `take`. This is a status policy, not proof of why the process received SIGPIPE.
It applies only to a connected downstream pipe; redirection can remove that connection.
Supervisor-sent termination signals used solely for cutoff cleanup may also be marked
expected, retaining their provenance. An unrelated nonzero exit or signal remains a
failure. Raw process pipelines do not automatically kill CPU-bound upstream programs
merely because a downstream program exits.

Ctrl-C is cancellation, not a successful short read. Ordinary functions cannot catch it
with `attempt`. Explicit `take` cutoff is successful data completion only if no real
error was already observed. A job that fails after producing bytes still fails when its
stream is finalized. EOF is not a substitute for `waitpid`.

Interrupting `wait` cancels the waiting evaluation and preserves the independently
started background job. Interrupting a foreground execution or an already requested
`cancel` waits for owned cleanup before returning to the prompt; no remaining expression
in that entry runs after the interrupt.

Interactive stop retains the live job and its execution continuation. The session
freezes scope-owned process groups with SIGSTOP and observes their stop or exit before
returning to the prompt; ordinary external terminal signals still follow POSIX job
control. `fg` resumes the retained groups and continuation. `bg` is supported only when
no in-process evaluator/stream continuation is attached; otherwise it raises a clear
capability error and leaves the job stopped. Other commands may execute while a context
is stopped. Already-launched jobs keep their launch snapshots and open resources remain
attached to the stopped scope. After resumption, explicit session-state reads and new
launches observe the current session cwd/environment; lexical captures remain unchanged.
User evaluators do not run concurrently in the background. `fg` returns
the resumed expression result without separately displaying it. Successful resumed
declarations publish a new REPL scope at completion, while their captured references
remain those resolved originally. The `fg` caller keeps its original lexical
environment; newly published names become available to the next entry. Nominal-name
conflicts discovered at publication are errors; earlier effects are not rolled back.
`wait` rejects handles with a suspended language continuation instead of implicitly
running it. A pure suspended evaluation uses the same handle surface but has no process
group. In a script, a stopped stream producer raises ProcessError and is cleaned up; it
cannot wait indefinitely for an interactive foreground request.

Control operations validate handle kind and state. `fg` can foreground an external job
or resume a stopped evaluation; a completed external job yields its checked result
without terminal handoff. `fg`, `wait`, and `cancel` reject control of the calling
evaluation or its attached jobs from within that evaluation; use stream ownership
operations to finish its data flow. Misuse cannot re-enter or destroy the active
evaluator. Separately started background jobs remain independently controllable.

## Stream ownership and scopes

Streams are single-consumer resources. Aliases share a control block identified by a
non-reused token; creating an alias does not clone the stream. A transformation consumes
its input token and returns a new token. A terminal sink consumes its token entirely.
Every use validates ownership; using an alias after transfer raises StreamConsumed.
`close stream` consumes and closes a stream without pulling more items. It returns Unit
after cleanup, not a claim of successful producer completion.

```rill
do {
  let input = files (path ".")
  let alias = input
  let output = input |> take 5
  collect alias # StreamConsumed: ownership moved into output
}
```

Borrowing an item while evaluating a callback does not grant ownership of the upstream
stream. A filter predicate can observe each item once without being able to recursively
drain the same input. Nested attempts to consume a busy stream fail.

An execution resource scope surrounds each top-level statement; an interactive entry may
contain several statements with one atomic binding publication. Ordinary function calls
share their caller's execution scope; returning a stream to the immediate caller is
valid. A stream may be stored locally or captured by a temporary closure inside that
scope. Before publishing REPL/module/script top-level bindings or returning a persistent
entry result, traverse the escaping graph and reject scoped resource handles, even if
hidden in a container or closure. Prefer `collect` or a reusable source function. The
traversal handles cycles in closure environments.

The interactive result renderer is a terminal consumer and may drain a stream before
publication. A script cannot silently discard a returned stream: it raises
UnconsumedStream and cleans it up. A discarded local stream is closed at scope exit; it
is not drained for hidden effects. Stream construction may already have launched a
process, so scope cleanup must cancel/reap it when necessary.

Scope exit on success, error, or cancellation closes all remaining resources in reverse
dependency order. Internal cleanup is idempotent; public stream operations still
validate their owner tokens. Cleanup does not wait for GC. A deliberately started
background job transfers ownership to the session and may escape as a Job handle.

## Consumption and materialization

External stages in a byte pipeline are started before waiting for completion. Bounded
queues provide backpressure. The default byte queue capacity is 64 KiB per edge; codecs
may retain an incomplete logical record up to their documented limit. `lines` limits one
line to 8 MiB by default; an explicit options function changes this. These are project
limits, not assumed kernel pipe capacities.

Byte streams yield nonempty Bytes chunks. Chunk boundaries have no text meaning. `lines`
incrementally decodes strict UTF-8 across chunks and emits String lines, stripping LF
and one preceding CR, preserving a final unterminated line, and not emitting a spurious
empty line after a final terminator. Empty input emits no lines. Binary data needs
explicit byte operations; invalid UTF-8 raises DecodeError.

`map`, `filter`, and `filter_map` preserve List versus Stream input category. Lists are
processed eagerly; streams remain lazy. `take` returns a List prefix or a stream with
cutoff. `drop` returns a List suffix or a lazy stream that discards the specified number
of items. `collect` accepts a value stream and returns a List; collecting bytes uses
`collect_bytes`. Neither scalar values nor nested lists are flattened implicitly.
`sort_by`, JSON document decoding, and collection are materialization boundaries and
enforce explicit limits. Default `collect` limits are 1,000,000 items and a 64 MiB
retained-representation budget; `seq.collect_with` changes them. The budget counts each
distinct backing object reachable from collected data once, including shared list
storage, and uses known backing capacities and owned payload sizes rather than a JSON
estimate. Private allocator metadata and collection bucket layouts are excluded; this is
a representation budget, not an RSS measurement. Functions remain valid collection
items; their reachable code and captured heap objects count once against the budget.
Session-owned OS state behind Job handles is accounted by the session, not copied into a
collection. Materialization does not extend a nested resource lifetime: a local
collection may contain Stream handles, but the same escape check applies if it is
published. Collecting does not recursively drain nested streams. Allocation sizes remain
checked independently of these budgets.

Callback order is deterministic. `map`/`filter` callbacks run once per visited item,
left-to-right. `sort_by` evaluates the key once per item before stable sorting. `take 0`
performs no callback work and closes its source. Scheduler progress and cancellation are
checked during long pure evaluations, not only on syscalls.

## Resource-owning producers

`seq.produce {acquire, step, release}` creates a resource-owning stream. Construction
validates the protocol; acquisition happens only on first demand:

| Callback  | Application                            | Result                                               |
| --------- | -------------------------------------- | ---------------------------------------------------- |
| `acquire` | `acquire ()`, on first demand          | Initial producer state                               |
| `step`    | `step state`, once per demand          | `Option.Some {value: [item, next]}` or `Option.None` |
| `release` | `release state reason`, during cleanup | Unit                                                 |

The step result has the same shape as `unfold`'s. The producer owns a child resource
scope. Successful acquisition and step results may retain resources in that scope;
yielded items still obey ordinary escape checks. Callback code may consume its
child-scope streams but cannot consume or adopt a stream owned by an outer scope.
Closing an undemanded producer performs no acquisition. If acquisition fails, unwind its
child scope without calling `release`. After successful acquisition, invoke `release`
once on normal exhaustion, close, cutoff, failure, cancellation, or enclosing scope
exit, then close remaining child resources.

Release receives the latest successfully returned state; failed or exhausted steps leave
the previous state current. The `seq.CloseReason` nominal type has cases `Exhausted`,
`Closed`, `Cutoff`, `Failed {error}`, and `Cancelled`. Scope exit uses `Closed`.
Stop/resume does not release the producer. The first transition to closing fixes the
reason and prevents re-entry.

Cleanup runs through normal VM continuations, with live state traced while suspended. A
pending operational error remains primary; cleanup errors are attached to it. With no
primary error, report the first cleanup failure. Cancellation remains cancellation.
Always attempt remaining cleanup after a failure, and do not let another ordinary
interrupt skip it. A release callback can catch its own ordinary errors with `attempt`;
the pending outer failure or cancellation is delivered after cleanup. Keep its original
source location, and display secondary cleanup failures even during cancellation. Forced
termination and a nonterminating user callback cannot carry an unconditional completion
guarantee. GC finalization never calls `release`.

## Multi-input streams

Use `zip inputs` and `merge inputs`, where `inputs` is a List of Streams. Validate all
tokens, scope relationships, and repeated identities before transferring any input. The
resulting stream owns all inputs and closes them on failure or downstream cutoff. An
empty input list produces an empty stream for both operations.

`zip` emits Lists in input order and stops at the shortest input. Pull left to right,
with at most one pending tuple. If a later input ends, earlier inputs may already have
yielded one unmatched item; discard it during closure. Do not prefetch the next tuple.

`merge` preserves each input's order and ends after every input completes. Its
cross-input order is unspecified. Poll ready inputs round-robin with at most one pending
item per input. Item bounds do not bound arbitrarily large payloads; retain applicable
value/materialization and byte-transport budgets too. Pending I/O on one input must not
block another ready input. Language callbacks remain serial and yield at VM checkpoints;
fairness cannot make a blocking native operation nonblocking.

For both operators, retain the first observed operational failure; use input order to
break ties observed in the same polling pass. Attempt cleanup in input order and retain
cleanup failures as secondary diagnostics. Byte EOF is not proof that an associated
producer or process completed successfully. Backpressure and cancellation must propagate
through the entire owned graph.

## Core library contracts

| Operation                                               | Contract                                                                              |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `map f sequence`                                        | Preserve category; one result per item                                                |
| `filter p sequence`                                     | Require Bool predicate; preserve category                                             |
| `fold_until f initial sequence`                         | Callback returns Control.Continue or Control.Stop; return the final payload           |
| `find p sequence`                                       | First matching item as Option; stop immediately after a match                         |
| `any p sequence`, `all p sequence`                      | Bool predicates, short-circuit; empty results are false and true respectively         |
| `filter_map f sequence`                                 | Callback returns Some to emit a value or None to omit it; preserve category           |
| `drop n sequence`                                       | Nonnegative Int; discard at most n items                                              |
| `chunks bytes`                                          | Explicit lazy stream of nonempty Bytes chunks; empty Bytes yields no items            |
| `items list`                                            | Explicit lazy, single-consumer view of a List                                         |
| `seq.produce protocol`                                  | Lazy acquisition, stateful demand and explicit release in an owned child scope        |
| `zip inputs`, `merge inputs`                            | Single-consumer composition of a List of Streams; see multi-input contracts above     |
| `unfold step initial`                                   | Lazy Stream; step returns None or Some `[item, next_state]`                           |
| `range start end`                                       | Lazy ascending Int stream, inclusive start and exclusive end; empty when start >= end |
| `fold f initial sequence`                               | Apply `f accumulator item` in order; return the final accumulator                     |
| `each f sequence`                                       | Run in order, discard callback values, return Unit                                    |
| `take n sequence`                                       | Nonnegative Int; successful early cutoff                                              |
| `close stream`                                          | Consume the handle and clean up without draining; return Unit                         |
| `sort_by key sequence`                                  | Stable order, homogeneous comparable keys, return List                                |
| `sum sequence`                                          | Homogeneous numeric kind; checked Int arithmetic                                      |
| `files path`                                            | Value stream of anonymous records from filesystem APIs                                |
| `glob pattern`                                          | Explicit filesystem expansion to List of Paths; sorted by path bytes                  |
| `read_text path`                                        | Read bounded content, decode strict UTF-8                                             |
| `from_json bytes_or_string`                             | One JSON document; return an ordinary value                                           |
| `to_json value`                                         | One JSON document as UTF-8 Bytes                                                      |
| `stdin ()`                                              | Scoped Bytes source borrowing standard input                                          |
| `write_bytes bytes`                                     | Write Bytes exactly; empty input is valid                                             |
| `write_text string`, `print string`                     | Write UTF-8 String exactly; print adds one LF                                         |
| `join_path base child`, `basename path`, `dirname path` | Lexical Path construction/decomposition without filesystem access                     |
| `write_stdout byte_stream`                              | Drain bytes exactly, then finalize associated jobs                                    |

`flat_map transform sequence` preserves the outer category. List input requires each
callback to return a List and concatenates those Lists eagerly; returning a Stream or
another kind raises TypeError. Use `items` explicitly to enter lazy composition.
Stream input accepts List or Stream results on demand. Exhaust and close each inner
sequence before advancing the outer one.
Cutoff, failure and cancellation close the active inner and outer resources; no later
callback is invoked. Streams retain their existing scope and single-consumer rules.

`group_by`, `count_by` and `unique_by` take a String-producing key function and a finite
sequence. Evaluate the key once per item in input order. Group/count return Records in
first-key order; groups preserve item order. Unique returns the first item for each key
as a List. Stream input first uses normal bounded collection; List input is already
materialized. These operations are explicit materialization boundaries, not online
infinite-stream aggregators.

`split_nul` and `text.split_nul_with {max_record_bytes}` split Bytes chunks on NUL into Bytes
records without UTF-8 decoding or CR stripping. Delimiters are excluded, adjacent
delimiters yield empty records, and a final unterminated nonempty record is emitted.
A trailing delimiter adds no extra record. Default record size is 8 MiB; overflow fails
rather than publishing a truncated record. Chunk boundaries do not change records.

`read_file path` opens a regular file and returns a scoped Bytes stream. `read_bytes`
and `fs.read_bytes_with {max_bytes}` explicitly collect it (64 MiB default).
`write_file path stream` truncates a regular file, consumes Bytes chunks and closes it;
`append_file` appends instead. Open/type errors occur before input is consumed. Paths
resolve against the session directory when opening, and live descriptors survive `cd`.
FIFOs, devices and directories are rejected; use explicit process plans for those
interfaces. Writes are not atomic or durable transactions, and earlier bytes remain on
failure. Never copy a file onto itself with a truncating sink.

`write_stderr` is the byte-stream counterpart of `write_stdout`; `eprint` accepts a
String and appends LF on stderr. `print` sends its text and LF in one acknowledged
transport frame, preserving exact bytes and cancellation without two round trips.

`fold_until` accepts only the shared Control constructors. Stop is normal consumer
cutoff, not an exception: it closes the upstream chain, waits for owned producers to
finish cleanup, and returns the payload. An already observed producer/callback error
still wins. Empty input returns the initial value without invoking the callback. `find`,
`any`, and `all` build on this mechanism, with no lookahead callback after a result is
known. Invalid callback results fail explicitly. List transformations remain eager and
ordered; combining separate transformations must not reorder their effects.

`unfold` keeps one state and at most one produced item rooted. It calls the step only on
demand; `take 0` closes it without invoking the callback. State may capture ordinary
values, but resource escape and single-consumer rules still apply. `items` supplies an
explicit List-to-Stream boundary; scalars and Lists never become streams implicitly.

`stdin ()` borrows descriptor 0 and never closes it or changes its status flags. At most
one live stdin source exists per session, including suspended evaluations. EOF, close,
cutoff, errors, and scope exit release its lease. Reading may buffer a chunk; closing
discards any already-read unconsumed bytes. A command inheriting stdin is rejected while
the lease exists; explicit input redirection, `through`, and sources whose stdin is
`/dev/null` remain usable. With `-c` or a source file, stdin is data; when the script
itself comes from stdin, source loading consumes it before evaluation.

```rill
stdin ()
  |> lines
  |> map { line => parse_int (trim line) }
  |> sum
  |> string
  |> print
```

`print` accepts String, never a debug rendering or an implicit conversion. Use `string`
for supported scalars, `display_path` for safe path display, and `to_json` for
structured data. Writes are cooperatively scheduled through the existing byte sink,
preserving cancellation and I/O errors.

Filesystem path parameters accept Path, String, or Bytes without NUL; operations return
Path where a path is produced. `join_path base child` inserts one separator when needed;
an absolute child replaces the base and an empty child preserves it. It does not
normalize `..`, resolve symlinks, or consult cwd. `basename` and `dirname` ignore
trailing separators, return `.` for empty input, and treat all-separator input as `/`; a
relative basename has dirname `.`. No locale or text decoding is applied.

`files` emits `name: Path`, `path: Path`, `kind: String`, and `size: Int`; `name`
contains the entry's basename and `path` the source-relative path. It includes hidden
entries but excludes `.` and `..`, does not recurse, uses no locale collation, and does
not promise directory iteration order. Kind describes the directory entry without
following a symlink; size follows the corresponding metadata semantics. Kind is one of
`file`, `directory`, `symlink`, or `other`. Open the source directory when `files` is
called and resolve subsequent metadata relative to that handle, so a suspended
evaluation is unaffected by another entry changing cwd. Metadata errors fail the stream
rather than silently omitting rows. `glob` accepts a String or Bytes pattern and
implements POSIX `*`, `?`, and bracket matching, with no brace expansion, recursive
`**`, or tilde expansion. A leading dot must be matched explicitly. No matches returns
an empty List; filesystem traversal errors fail rather than silently hiding permission
problems. Disable libc locale sorting and sort resulting Path bytes explicitly.

JSON input is strict: reject duplicate keys, invalid UTF-8, comments, trailing commas,
non-finite numbers, and integer tokens outside Int range. Real-number tokens become
finite Float. Encoding supports Null, Bool, Int, Float, String, List, and anonymous
Record; reject Unit, nominal ADTs, Path, Bytes, Function, plans, and resources with a
precise value path. Conversion is explicit, never a placeholder string. Default document
limits are 64 MiB of input/output and 256 levels of nesting; explicit options functions
may change them. Integer tokens cannot silently become Float through a library overflow
fallback. Encoding integral-valued Floats retains a decimal point or exponent so
decoding does not silently change their kind.

### Explicit limit options

Options are ordinary anonymous records. Omitted known keys use the documented built-in
defaults; unknown keys and negative/non-Int limits are errors. There is no implicit
unlimited sentinel. Zero permits only an empty result for size/count limits; nesting
depth must be at least one.

| Operation                                                           | Option keys              |
| ------------------------------------------------------------------- | ------------------------ |
| `process.capture_with options pipeline`                             | `max_bytes`              |
| `seq.collect_with options stream`                                   | `max_items`, `max_bytes` |
| `seq.collect_bytes_with options stream`                             | `max_bytes`              |
| `text.lines_with options byte_stream`                               | `max_line_bytes`         |
| `text.split_nul_with options byte_stream`                           | `max_record_bytes`       |
| `fs.read_text_with options path`, `fs.read_bytes_with options path` | `max_bytes`              |
| `json.from_json_with options input`                                 | `max_bytes`, `max_depth` |
| `json.to_json_with options value`                                   | `max_bytes`, `max_depth` |
| `seq.sort_by_with options key sequence`                             | `max_items`, `max_bytes` |

The unqualified functions partially apply their configurable counterparts to empty
options Records. This binds defaults without executing the operation. `collect_bytes`
and `read_text` default to 64 MiB; sorting uses collection limits. `seq.sort_by_with`
applies limits to List input as well as Stream input. Its retained-byte budget includes
input values and cached keys together, counting shared objects once. Check byte/count
budgets as data is consumed or retained and before output is emitted. Bounded lookahead
may detect an excess; it does not permit publishing truncated success. Decoded JSON
depth is checked during conversion, before publishing a value. Limits apply to the named
operation, not total process memory. Earlier external effects, including written bytes,
are not rolled back.

## Session state and shutdown

`cd path`, `set_env name value`, and `unset_env name` are ordinary foreground session
effects, including when called by stream callbacks. They affect later reads and
launches, never mutate an already-launched job's snapshot. There is no hidden
callback-specific effect restriction. Callback order determines when the effect occurs;
background jobs execute no user-language callbacks. `pwd ()` returns a Path.

Job handles use stable, non-reused session IDs, never bare PIDs. Stream aliases use
permanent consumption checks to detect ownership transfer. Completed reports remain
available through retained handles. `jobs ()` returns records containing `id`, `handle`,
`kind`, and `state`, so even a discarded start result can be recovered through the
session. Dropping a handle does not detach a process. `get_env name` returns Option of
Bytes. Environment names are nonempty Strings without NUL or `=`; `set_env` values are
String or Bytes without NUL. `unset_env name` removes a variable explicitly. `exit code`
refuses while jobs are live or stopped; Ctrl-D on an empty prompt makes the same request
with code zero. `exit_force code` explicitly cancels session jobs and restores the
terminal before exit. There is no detach/disown operation. Script EOF with
unacknowledged background jobs is an error followed by cleanup. Scripts must explicitly
wait, foreground, or cancel each job, even if the event loop already reaped its
children. Merely listing a job does not acknowledge its completion.

Cancellation closes relevant channels, sends SIGTERM to owned live groups, and sends
SIGCONT to stopped groups. After a 1-second grace period, send SIGKILL if needed. The
supervisor continues nonblocking I/O and reaping until owned children are accounted for;
the escalation deadline does not guarantee kernel termination by that instant. Expected
cutoff uses the same cleanup policy when closing channels is insufficient.

Ownership covers launched children and signaling their existing groups, not a portable
process-tree containment guarantee. Descendants can leave a group. Never signal a
numeric group ID after the last owned member has been reaped; reports retain data, not
signaling authority. Cleanup and terminal restoration remain explicit even when an
unrelated process keeps a pipe open.

Script exit is 0 on normal completion, 1 for ordinary unhandled language/I/O errors, 2
for syntax/CLI errors, 130 for user interrupt, or the selected checked external stage's
nonzero code. A policy failure with actual exit zero maps to 1. Signal failures map to
the smaller of 255 and `128 + signal` while reports retain the actual signal. Explicit
`exit` accepts codes 0 through 255. Allocator exhaustion is fatal and may abort without
running destructors or release callbacks. Operation limits reduce retained data; they do
not guarantee recovery from process-wide allocation failure.
