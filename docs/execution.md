# Execution

This document specifies the first-release target for processes, streams, and effectful
library operations. Application order follows [language](language.md).
[Current status](status.md) records current support.

## Plans, jobs, reports, and streams

| Object    | Meaning                                         | Lifetime                       |
| --------- | ----------------------------------------------- | ------------------------------ |
| JobPlan   | Immutable external pipeline description         | Ordinary GC-managed value      |
| Job       | Process group, children, descriptors, state     | Explicit session/job ownership |
| JobReport | Immutable completion data for all stages        | Ordinary GC-managed value      |
| Stream    | Single-consumer traversal with possible failure | Explicit execution scope       |

A JobPlan contains evaluated, validated command arguments and can be launched repeatedly.
Each launch creates a distinct Job. A JobReport retains completion data without granting
authority to signal a process. Plans retain execution descriptions, not unevaluated source.

## Commands and plan construction

```rill
let message = "hello world"
let paths = [path("a.txt"), path("b c.txt")]

^printf "%s\n" $message
^cat ...$paths

let subjects = job {
  ^git log -n 100 --format=%s | ^sort
}
run(subjects)
```

`job { ... }` contains exactly one external pipeline, with optional newlines around `|`.
It constructs a plan. A command statement beginning with `^` outside this form lowers to
`run` of that plan and returns Unit on success. Commands are statements, not general
expressions; use `job` and an execution function inside expressions. The process pipe
`|` is only legal in this command grammar. `|>` cannot be attached directly to an
executing command statement: an explicit stream/capture bridge is required. A leading
process pipe on a following line continues a pipeline only while that source is still
buffered; use a trailing `|` to request continuation before an interactive entry is
submitted.

Command words use these rules:

- The word immediately after `^` identifies the executable. It can be literal or
  a scalar substitution; no alias or language-function fallback is performed.
- An unquoted word is literal text. `*`, `?`, and `~` have no expansion behavior.
- Quoted words use the language's string rules and never interpolate internally.
- `$name` and `$(expression)` each contribute exactly one String, Path, or Bytes
  argument. NUL is rejected. Other types require explicit conversion.
- `...$name` and `...$(expression)` require a List of valid scalar arguments and
  contribute one argument per element, including empty arguments. An empty list
  contributes none. Spreading a stream is rejected.
- Adjacent literal, quoted, and substituted fragments without whitespace are
  rejected; use string concatenation inside `$(...)` for a single composed word.
- Whitespace separates words. Unquoted `|`, `;`, braces, and redirection operators
  are syntax and must be quoted to be literal. A newline ends a complete command.
- There is no implicit globbing, command substitution, backtick syntax, escape-based
  line continuation, word splitting, or removal of trailing newlines.

Plan arguments and redirection paths evaluate left-to-right during construction,
including across stages. Creating a plan does not launch its described processes or open
its redirection files. Embedded expressions are ordinary strict expressions and may
themselves have effects; plan syntax does not make them pure.

`command(executable, arguments)` constructs the same one-stage plan programmatically.
`pipe(left, right)` composes plans and preserves their per-stage policies. There is no
AST re-parsing. `with_cwd(path, plan)` and `with_env(record, plan)` return new plans
with overrides applied to every stage. Repeated overrides replace the same key; other
stage settings remain intact. Environment overrides overlay the launch snapshot and use
the same name/value validation as `set_env`.

Unspecified cwd, environment, and PATH are snapshotted at launch. Explicit cwd overrides
are resolved to an absolute directory at plan construction so later `cd` does not
reinterpret them. Relative command arguments and redirection paths remain relative to
the launch cwd. Unqualified executables are found using that launch's PATH. Reusing a
plan therefore does not promise identical external state or output.

## Redirection

The first release supports `<`, `>`, `>>`, `2>`, `2>>`, and `2>&1`. Other descriptor
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

| Function         | Behavior                                                                                                |
| ---------------- | ------------------------------------------------------------------------------------------------------- |
| `run(plan)`      | Foreground execution, inherited terminal streams; check all stage policies; return Unit                 |
| `capture(plan)`  | Foreground execution, concurrent stdout/stderr capture; return report and Bytes; nonzero exit is data   |
| `stream(plan)`   | Foreground job source producing stdout Bytes chunks; stderr inherited; completion checked at stream end |
| `start(plan)`    | Start external-only background job; return session Job handle                                           |
| `wait(handle)`   | Wait for a background job's report; nonzero exit is data; a stopped job raises JobStopped               |
| `check(report)`  | Reject cancelled or failed reports; otherwise Unit                                                      |
| `fg(handle)`     | Resume a job/context; return Unit for an external-only checked job or the resumed evaluation's result   |
| `bg(handle)`     | Resume a stopped external-only job without terminal ownership; return Unit                              |
| `cancel(handle)` | Cancel and await cleanup; return Unit; completed jobs are unchanged                                     |
| `jobs()`         | Immutable snapshots of session jobs                                                                     |

`start`, `stream`, and `through` initiate launch when called. They complete the launch
handshake before returning a handle; setup/exec error records raise a launch error. A
returned handle is not a promise of successful process termination. Transforming a
stream is lazy with respect to pulling items, not with respect to effects already
performed while constructing its source.

`capture` returns a record with `stdout`, `stderr`, and `report` fields. It preserves
bytes exactly, including trailing newlines and NUL. Launch failures and capture resource
limits still raise errors; a command that starts and exits nonzero produces data.
Capture has a default combined output limit of 64 MiB; `capture_with(options, plan)` can
set a larger explicit limit. Exceeding the limit cancels/reaps the job and raises
LimitExceeded with counts, never returns silently truncated success.

Absent explicit redirection, `capture` and `stream` use `/dev/null` for stdin so a data
source does not unexpectedly read from the user's terminal. `run` inherits stdin.
`start` defaults stdin to `/dev/null` and inherits stdout/stderr; background output may
appear while editing. Explicit redirection overrides these defaults.
`capture`, `stream`, and `through` are foreground evaluations but keep the shell in
the terminal foreground group; the supervisor forwards terminal signals to their attached process
groups. An inherited stderr TTY destination is relayed through a bounded supervisor
pipe, so data-job groups do not stop merely because the terminal has TOSTOP enabled.
Non-TTY inherited stderr may be connected directly. Programs requiring direct access to
the controlling terminal must use `run`.

`through(plan, byte_stream)` is the streaming process sink/source: it feeds the first
stage's stdin and exposes the last stage's stdout while inheriting stderr. It
concurrently pumps input and output and checks completion. Conflicting explicit stdin or
stdout redirection is rejected for `through` instead of discarding the supplied stream.
Normal exit, downstream cutoff, and errors propagate upstream.

```rill
stream(job { ^git log --format=%s })
  |> lines
  |> filter(starts_with("fix:"))
  |> take(10)
  |> collect

files(path("."))
  |> map(fn(e) => {name: display_path(e.name), size: e.size})
  |> collect
  |> to_json
  |> chunks
  |> through(job { ^cat })
  |> write_stdout
```

`to_json` encodes one materialized value to Bytes. `chunks` turns Bytes into a byte
stream. NDJSON is outside the initial codec surface; it is never inferred from JSON text
or arbitrary process chunk boundaries.

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

Default accepted exit codes are `[0]`. `accept_exit(codes, plan)` replaces the final
stage's policy with a nonempty List of codes in 0–255. Configure an earlier stage before
composition. Reports retain these policies and actual terminations after expected-cutoff
classification; they never rewrite a signal into exit zero.

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

Interactive stop retains the live job and its execution continuation. `fg` resumes both.
`bg` is supported only when no in-process evaluator/stream continuation is attached;
otherwise it raises a clear capability error and leaves the job stopped. Other commands
may execute while a context is stopped. Already-launched jobs keep their launch
snapshots and open resources remain attached to the stopped scope. After resumption,
explicit session-state reads and new launches observe the current session
cwd/environment; lexical captures remain unchanged. No user evaluator runs concurrently
in the background in the first release. `fg` returns the resumed expression result without separately
displaying it. Successful resumed declarations publish a new REPL scope at completion,
while their captured references remain those resolved originally. Nominal-name conflicts
discovered at publication are errors; earlier effects are not rolled back. `wait`
rejects handles with a suspended language continuation instead of implicitly running it.
A pure suspended evaluation uses the same handle surface but has no process group.

Control operations validate handle kind and state. `fg` can foreground an external job
or resume a stopped evaluation; a completed external job yields its checked result
without terminal handoff. `fg`, `wait`, and `cancel` reject control of the calling
evaluation or its attached jobs from within that evaluation; use stream ownership
operations to finish its data flow. Misuse cannot re-enter or destroy the active
evaluator. Separately started background jobs remain independently controllable.

## Stream ownership and scopes

Streams are single-consumer resources. Aliases share a control block and an owner token
with a generation number; creating an alias does not clone the stream. A transformation
consumes its input token and returns a new token. A terminal sink consumes its token entirely.
Every use validates ownership; using an alias after transfer raises StreamConsumed.
`close(stream)` consumes and closes a stream without pulling more items. It returns
Unit after cleanup, not a claim of successful producer completion.

```rill
do {
  let input = files(path("."))
  let alias = input
  let output = input |> take(5)
  collect(alias)  # StreamConsumed: ownership moved into output
}
```

Borrowing an item while evaluating a callback does not grant ownership of the upstream
stream. A filter predicate can observe each item once without being able to recursively
drain the same input. Nested attempts to consume a busy stream fail.

An execution scope surrounds each top-level entry/statement. Ordinary function calls
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

`map` and `filter` preserve List versus Stream input category. Lists are processed
eagerly; streams remain lazy. `take` returns a List slice or a stream with cutoff.
`collect` accepts a value stream and returns a List; collecting bytes uses
`collect_bytes`. Neither scalar values nor nested lists are flattened implicitly.
`sort_by`, JSON document decoding, and collection are materialization boundaries and
enforce explicit limits. Initial `collect` limits are 1,000,000 items and a 64 MiB
retained-representation budget; `collect_with` changes them. The budget counts each
distinct backing object reachable from collected data once, including shared list
storage, and uses runtime allocation sizes rather than a JSON estimate. Functions remain
valid collection items; their reachable code and captured heap objects count once
against the budget. Session-owned OS state behind Job handles is accounted by the
session, not copied into a collection. Materialization does not extend a nested resource
lifetime: a local collection may contain Stream handles, but the same escape check
applies if it is published. Collecting does not recursively drain nested streams.
Allocation sizes remain checked independently of these budgets.

Callback order is deterministic. `map`/`filter` callbacks run once per visited item,
left-to-right. `sort_by` evaluates the key once per item before stable sorting.
`take(0)` performs no callback work and closes its source. Scheduler progress and
cancellation are checked during long pure evaluations, not only on syscalls.

## Core library contracts

| Operation                    | Contract                                                             |
| ---------------------------- | -------------------------------------------------------------------- |
| `map(f, sequence)`           | Preserve category; one result per item                               |
| `filter(p, sequence)`        | Require Bool predicate; preserve category                            |
| `fold(f, initial, sequence)` | Apply `f(accumulator, item)` in order; return the final accumulator  |
| `each(f, sequence)`          | Run in order, discard callback values, return Unit                   |
| `take(n, sequence)`          | Nonnegative Int; successful early cutoff                             |
| `close(stream)`              | Consume the handle and clean up without draining; return Unit        |
| `sort_by(key, sequence)`     | Stable order, homogeneous comparable keys, return List               |
| `sum(sequence)`              | Homogeneous numeric kind; checked Int arithmetic                     |
| `files(path)`                | Value stream of anonymous records from filesystem APIs               |
| `glob(pattern)`              | Explicit filesystem expansion to List of Paths; sorted by path bytes |
| `read_text(path)`            | Read bounded content, decode strict UTF-8                            |
| `from_json(bytes_or_string)` | One JSON document; return an ordinary value                          |
| `to_json(value)`             | One JSON document as UTF-8 Bytes                                     |
| `write_stdout(byte_stream)`  | Drain bytes exactly, then finalize associated jobs                   |

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

| Operation                              | Option keys              |
| -------------------------------------- | ------------------------ |
| `capture_with(options, plan)`          | `max_bytes`              |
| `collect_with(options, stream)`        | `max_items`, `max_bytes` |
| `collect_bytes_with(options, stream)`  | `max_bytes`              |
| `lines_with(options, byte_stream)`     | `max_line_bytes`         |
| `read_text_with(options, path)`        | `max_bytes`              |
| `from_json_with(options, input)`       | `max_bytes`, `max_depth` |
| `to_json_with(options, value)`         | `max_bytes`, `max_depth` |
| `sort_by_with(options, key, sequence)` | `max_items`, `max_bytes` |

The unqualified functions are ordinary wrappers supplying empty options records.
`collect_bytes` and `read_text` default to 64 MiB; sorting uses collection limits.
`sort_by_with` applies limits to List input as well as Stream input. Its retained-byte
budget includes input values and cached keys together, counting shared objects once.
Check byte/count budgets as data is consumed or retained and before output is emitted.
Bounded lookahead may detect an excess; it does not permit publishing truncated success.
Decoded JSON depth is checked during conversion, before publishing a value. Limits
apply to the named operation, not total process memory. Earlier external effects,
including written bytes, are not rolled back.

## Session state and shutdown

`cd(path)`, `set_env(name, value)`, and `unset_env(name)` are ordinary foreground
session effects, including when called by stream callbacks. They affect later reads and
launches, never mutate an already-launched job's snapshot. There is no hidden
callback-specific effect restriction. Callback order determines when the effect occurs;
background jobs execute no user-language callbacks. `pwd()` returns a Path.

Job handles use stable session IDs and generation checks, not reusable bare PIDs.
Completed reports remain available through retained handles. `jobs()` returns records
containing `id`, `handle`, `kind`, and `state`, so even a discarded start result can be
recovered through the session. Dropping a handle does not detach a process.
`get_env(name)` returns Option of Bytes. Environment names are nonempty Strings without
NUL or `=`; `set_env` values are String or Bytes without NUL. `unset_env(name)` removes
a variable explicitly. `exit(code)` refuses while jobs are live or stopped; Ctrl-D on an
empty prompt makes the same request with code zero. `exit_force(code)` explicitly
cancels session jobs and restores the terminal before exit. There is no detach/disown
operation. Script EOF with unjoined background jobs is an error followed by cleanup; scripts must explicitly wait, foreground, or cancel. Here unjoined means not
acknowledged by one of those operations, even if the event loop already reaped all
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
`min(255, 128 + signal)` while reports retain the actual signal. Explicit `exit` accepts
codes 0 through 255. Allocation failure is fatal after best-effort non-allocating cleanup
and produces a nonzero exit.
