# Implementation status

Stages 1–3 are complete. Rill Shell combines external pipelines with its functional
language, reusable job plans, scoped structured streams, filesystem/JSON adapters, and
resumable foreground evaluations. The [implementation plan](implementation-plan.md)
reserves rich editing, completion, persistent history, and final delivery work for stage
4. The specifications describe the full first-release target; this page separates
implemented behavior from that remaining scope.

## Available surface

The executable accepts `-c SOURCE`, a source file with arguments, redirected stdin, or
interactive canonical input. Complete entries are parsed before evaluation; incomplete
entries continue at the prompt. Invocation supports `--`, `-i`, `--help`, `--version`,
color overrides, and `--no-config`. Interactive startup loads XDG configuration; scripts
receive argument Bytes through `args()`.

The language provides checked arithmetic, distinct String/Bytes/Path values, immutable
Lists and Records, first-class curried closures, proper tail calls, nominal structs and
enums, nested patterns and guards, immutable updates, and explicit errors. Closures
retain resolved free bindings. Failed entries publish no bindings but preserve completed
effects. Imports cache file identity, preserve nominal identity across aliases, reject
cycles, and permit retry after failure. Relative imports use the importing directory.

The prelude and `std:core`, `std:seq`, `std:text`, `std:fs`, `std:process`, and
`std:json` are embedded through C23 `#embed`. Composition and defaults use ordinary Rill
functions over private primitives; the installed shell needs no library source files or
Python.

| Surface | Implemented behavior |
| --- | --- |
| Commands and plans | Literal/quoted words, scalar/list-spread substitutions, byte pipelines, ordered redirection, reusable and programmatic plans |
| Job policies | Launch snapshots, explicit cwd/environment overrides, accepted exit codes, stage reports, expected cutoff, cancellation |
| Process APIs | `run`, `start`, `wait`, `check`, `capture`, `stream`, `through`, `fg`, `bg`, `cancel`, `jobs` |
| Streams | Single-consumer ownership, lazy `map`/`filter`/`take`, explicit close, checked producer completion, bounded byte queues |
| Consumers | `collect`, `collect_bytes`, `fold`, `each`, `sum`, stable `sort_by`, `write_stdout`; explicit materialization limits |
| Text and JSON | UTF-8 line decoding across chunks; strict document conversion with duplicate-key, numeric-range, depth, and byte checks |
| Filesystem | Directory records, byte-sorted explicit globbing, bounded regular-file text reads, safe Path display |
| Session | Explicit cwd/environment effects; suspended pure/mixed evaluations, foreground resumption, shutdown cleanup |

Returned streams display incrementally at the prompt. Scalars use escaped display and
containers currently use summaries; Unicode-aware tables belong to stage 4. The prompt
uses canonical terminal editing. RGB, 256-color, 16-color, and plain styles are
available. Unicode 18.0.0 grapheme segmentation is implemented; full terminal-width
validation, rich editing, completion, and history remain pending.

## Runtime boundaries

The prepared-AST evaluator uses exact lexical captures, proper tail calls, immutable
binding snapshots, and a precise nonmoving collector. Native roots also retain stopped
contexts. OS resources have explicit scopes and never rely on GC finalizers.

One cooperative loop advances the active evaluator, jobs, and bounded transport.
Suspended contexts resume only in the foreground. Stream aliases are invalidated on
transfer; escaping resource tokens are rejected even after close. Cleanup finishes
before error recovery resumes. See [architecture](architecture.md) for storage and
scheduling, and [execution](execution.md) for limits and observable policies.

## Acceptance evidence

There are 31 ordinary Meson tests and three optional fuzz seed-replay tests. C component
and fault tests and Python script/process/PTY tests exercise the production
implementation. Counts describe the current suite, not coverage percentages.
[Testing](testing.md) owns the detailed contracts and reproduction commands.

| Contracts | Evidence |
| --- | --- |
| L01–L04 | Syntax matrices, unary application, value-pipe evaluation order |
| L05–L08 | Lexical captures, retained closures, million-call tail scenarios, stress GC, retained-heap plateau checks |
| L09–L12 | Checked arithmetic/conversions, distinct empty values, Result conversion, script/REPL boundaries, uncaught cancellation |
| L13–L16 | Module identity/cycles/retry, atomic publication, exact captures around local Streams, callback/session effect order |
| A01–A12 | Nominal identity, first-class constructors, patterns and guards, immutable updates, equality, shared List tails |
| E01–E08 | Plan reuse/snapshots, argument bytes, redirections, launch distinction, large concurrent stdin/stdout/stderr transport |
| E09–E14 | Cutoff, failure after byte EOF, alias/reentrancy rejection, resource escape, limits, checkpoint cleanup, failed launches/codecs |
| E15–E20 | Descriptor/signal contracts, pure and mixed stop/resume, multiple contexts, cancellation, rejected background execution, forced shutdown |
| P01–P04, P07, P13–P14 | PATH/environment, directory transactions, XDG startup, UTF-8 boundaries, invocation, terminal ownership |
| P08–P09 | Color selection and canonical-input fallback; rich-terminal selection remains stage 4 |
| I02 | All 853 official grapheme cases and deterministic regeneration; no full display-width claim |

Stream regressions include no callback lookahead after `take`, nested consumers,
repeated scope release, strict JSON round trips, UTF-8 across transport boundaries,
directory identity across suspension and `cd`, resumed errors and nominal conflicts,
TOSTOP stderr relay, and cleanup before caught-error recovery. Allocation sweeps fail
one allocation at a time under stress GC, including stream callbacks/checkpoints and
JSON conversion. Separate fuzz targets check JSON conversion, line-decoder
content/errors, and syntax preparation without executing arbitrary input, launching
processes, or touching user files.

Cleanup evidence concerns owned children and descriptors. It does not establish
containment of descendants that leave the process group. Cooperative scheduling does not
bound blocking filesystem calls, inherited-destination writes, GC pauses, or kernel
termination latency.

## Validation checkpoint

The 2026-09-18 C-audit checkpoint passed **31/31** ordinary Meson tests in each profile
below. Versions identify tested configurations, not requirements.

| Platform | Profiles |
| --- | --- |
| Linux/glibc, AArch64 | Clang 23.1.0 ASan/UBSan; GCC 16.2.1 release |
| macOS, arm64 | Clang 23.1.1 ASan/UBSan and release |

Both ordinary sanitizer builds passed clang-format, clang-tidy, Doxygen, Unicode
regeneration, and tests. System coverage includes 12 language, 19 process/invocation, 14
stream/process, four codec, and 23 PTY methods. Python sources are unchanged since the
test-review checkpoint, when Ruff formatting/lint and ty passed. Linux sanitizer runs
include leak checking.

All 24 authored headers also compile independently on macOS. The three optional fuzz
seed tests pass on macOS and Linux. ASan/UBSan mutation campaigns completed without a
finding:

| Target | macOS executions / seconds | Linux executions / seconds |
| --- | --- | --- |
| JSON | 750,691 / 11 | 1,062,899 / 11 |
| Lines | 31,458 / 11 | 90,541 / 11 |
| Syntax/preparation | 975,555 / 11 | 1,485,448 / 11 |

Campaigns used a 128 KiB mutation limit and a ten-second per-input timeout. Linux used
libFuzzer's leak checking; macOS disabled it. Components and yyjson received coverage
instrumentation. These are bounded runs, not coverage percentages or throughput
comparisons: the targets perform different work and scheduling was not isolated.

Stress-GC and fault regressions cover partial builders, exact captures, recursive
factories, pooled literals, Record indexes, suspended contexts, checkpoint cleanup, and
failed-entry recovery. Abort also clears borrowed diagnostics before collection and
permits subsequent definitions. Generated tables and dependencies are not hand-edited.

Remaining delivery checks include x86-64, macOS LeakSanitizer, rich-editor emulators,
and the planned editor and bounded-evaluator fuzz targets. Stage-4 contracts are not
inferred from passing stream or language tests.

## Performance evidence

All 23 release benchmarks pass on macOS/Clang and Linux/GCC. They validate workloads
without timing thresholds. On the tested AArch64 ABIs, Values occupy 16 bytes, object
headers 64 bytes, and syntax nodes 88 bytes; these are observations, not ABI promises.

Prior measurements motivated nested free-name summaries and code-local constant sharing.
Sharing saves repetitive storage but adds preparation work for distinct literals. Those
comparisons used earlier workload revisions and are not a current performance baseline.
No bytecode prototype, cache-miss improvement, allocator-traffic reduction, or bounded
GC pause has been measured. [Architecture](architecture.md#optimization-policy) records
the retained design and tradeoffs; [testing](testing.md#performance-evidence) defines
how to produce a reproducible comparison with the current workloads.
