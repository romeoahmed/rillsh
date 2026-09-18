# Testing

This document defines test infrastructure and acceptance contracts for the first
release. [Current status](status.md) records current coverage and remaining work. Tests
verify observable behavior, ownership, and failure recovery. Component checks may
inspect representation when it is part of an ownership or memory-safety contract.

## Test infrastructure

[Meson](https://mesonbuild.com/Unit-tests.html) owns executable isolation, parallelism,
deadlines, selection, repetition and JUnit logs. C tests use one always-active `CHECK`
macro: it evaluates its condition once and
calls `exit(EXIT_FAILURE)` on failure, preserving `atexit` cleanup. It remains active
under NDEBUG and leaves sanitizer signal handlers intact. Keep registration, reporting,
and scheduling in Meson. Runtime heap/equality/entry scenarios and language
semantics/code/collection/tail scenarios run as separate Meson tests, as do supervisor
scenarios. Each reuses its component executable; no second test runner is needed. The
million-call tail workload has its own deadline and can be selected independently.

Python's standard-library [unittest](https://docs.python.org/3/library/unittest.html)
owns system/PTY fixtures and assertions; Meson owns their outer process lifecycle.
The generator's Python unit tests run through the same discovery mechanism, without
shell fixtures. They check property parsing, range normalization against a pointwise
model, conflicting overlaps, byte-exact input hashes, and nonmutating CLI checks
(including newline drift). Deterministic regeneration and the official grapheme corpus
independently check the resulting tables.
Shared Rill predicates use one scoped `assert_rill`
helper; tests that inspect output or errors use the subprocess result directly.
Keep the C assertion layer small; add a test library only when fixture reuse or
diagnostics justify another dependency.

Pure library tables run directly under stress GC; each case has a local language scope.
System tests concentrate on executable invocation, native effects, files, environment,
and terminal behavior rather than repeating pure-value tables. Error cases check the
specific category and, at the executable boundary, the exit status. Native codes must
describe the failing operation; module tests seed an unrelated `errno` to verify that
semantic failures do not inherit it. Failure diagnostics identify the input, expected
result, and observed outcome. Use Unicode escapes in
authored test strings while retaining real non-ASCII bytes at runtime; upstream corpora
stay intact.

Assert observable behavior and documented component contracts, not closure layouts,
interned addresses, collector object counts, or scheduling quanta. Diagnostic tests
check categories and useful context; prompt tests may recognize a short notification,
but should not snapshot entire paragraphs. White-box checks remain appropriate for roots, nonmoving addresses, accounting, and
private builders. Compare retained space across repeated or scaled workloads instead of
imposing fixed byte ceilings. Test deadlines bound hangs, not performance.

### Fixtures and failure paths

C helpers expose bytes, environment, cwd, descriptors, simultaneous stdout/stderr,
signals, terminal state and exit behavior. Use them instead of GNU/BSD utility-specific
assumptions. Descriptor checks enumerate `/dev/fd` on both supported systems, excluding
the enumeration's own descriptor. A fixture at descriptor 256 or higher exercises
high-numbered descriptors when permitted; under a lower `RLIMIT_NOFILE` soft limit, it
uses the highest permitted descriptor number without raising the limit. Readiness cases
cover data, empty pipes, EOF, and closed descriptors on both sides of the historical
`FD_SETSIZE` boundary when permitted. PTY regressions also cover reading source and
streaming through `/dev/tty`. A controlling-PTY case inherits enough descriptors
to place the shell's terminal above that boundary; only this case is skipped when the
inherited soft limit cannot accommodate the fixture.

A private archive compiles the actual source/text/parser, runtime, library, directory
transaction and launch code with test-only allocation/POSIX aliases, including
`strndup` for identifier storage. Allocation budgets
advance until the first complete success, checking each failed prefix for the correct
error and preserved ownership. Meson bounds each sweep; allocation counts are not test
limits. Library and module scenarios also fail exactly one allocation and then restore
the allocator, exposing swallowed errors that persistent exhaustion can conceal. They
run under stress GC and cover arguments, immutable plan overrides, byte conversion,
record-rest matching, error conversion, sorting, Unicode scalar splitting, export tables, and
cancelled-job reports. Module construction must report allocation failure before
emitting a completion event and preserve earlier bindings. Named launch scenarios cover
first/later fork and registration failure, stopped unregistered children and termination
before exec. These substitutions remain outside production interfaces. They are not
exhaustive OS or allocator fault coverage.

Lifecycle cases destroy source storage before using prepared code, unregister roots out
of order, shrink private builders, collect shared List backing storage, and abort failed
evaluations before defining new bindings. Each verifies the documented lifetime or
failure guarantee under stress GC. Capture cases compare ordered values across multiple
closure instances, shadowing, nominal patterns, and narrow/wide layouts. A suspended
entry test publishes over both intervening snapshots and native definitions while
retaining the original closure bindings. Module fixtures reject non-regular files without
waiting for a FIFO writer. Signal fixtures cover inherited masks, full wakeup pipes,
errno preservation, restoration, and noninteractive terminal isolation.

Supervisor scenarios run in separate Meson processes. Their `atexit` cleanup kills only
known unreaped helper children and bounds the remaining supervision wait. A deliberate
exit-42 scenario verifies cleanup of a live job; `expected_exitcode: 42` rejects
unrelated assertion failures and cleanup failure (99). Python fixtures register
temporary-directory and PTY cleanup through `enterContext`, including failures during
`setUp`. Ordinary subprocesses get a cooperative shutdown interval, then bounded forced
termination. Pipe cleanup uses `ExitStack`; subprocess reaping has an explicit deadline
rather than an unbounded `Popen` context-manager wait. PTY fixtures also reclaim the
foreground group. No portable cleanup mechanism promises to recover arbitrary escaped
descendants after the supervisor itself crashes.

System tests use unittest's native `test_*.py` discovery and `subTest` for input
matrices. Meson passes built executable locations through two test-only environment
variables and explicit `depends` targets, so selected suites rebuild their helpers. Each
case owns its HOME/XDG directories, locale and color hints. Preserve sanitizer
environment settings; an empty environment can disable symbolizer lookup or change error
reporting. PTY tests wait for actual output, bound retained output, and use deadlines
for reads, writes and reaping. Interrupt tests wait for an executed marker and shell
foreground ownership, not terminal echo or a startup sleep. Short polling waits are for
OS state observation, never substitutes for readiness.

Use Meson's native selection and repetition instead of hard-coded retry loops:

```sh
meson test -C build --print-errorlogs
meson test -C build --suite runtime
meson test -C build language-tail --print-errorlogs
meson test -C build 'supervisor-*' --repeat=10 --logbase=repeat --print-errorlogs
meson test -C build --suite pty --print-errorlogs
```

Current Meson suites are `unicode`, `syntax`, `runtime`, `language`, `editor`,
`process`, `platform`, `fault`, `pty`, `stream`, and `codec`, plus optional `fuzz`. The
language suite covers closures, ADTs, modules, and pure library operations. PTY
unavailability is an explicit failure in the required Linux/macOS matrix. Tests must not
claim raw-editor or full Unicode width coverage before those features exist. The `codec`
suite isolates JSON and line conversion from the `stream` suite's resource ownership and
process transport scenarios. Both use the executable boundary.

## Language conformance

| ID  | Contract                                                                                          |
| --- | ------------------------------------------------------------------------------------------------- |
| L01 | Lexer distinguishes command words from expressions without name-dependent parsing                 |
| L02 | Complete/Incomplete/Invalid agree across strings, comments, blocks, pipes, and EOF                |
| L03 | Whitespace application equals staged unary application, including intermediate effects            |
| L04 | `x \|> f` evaluates x first, once; hygienic lowering cannot capture user names                    |
| L05 | Closures survive defining entries; later shadowing does not change captured bindings              |
| L06 | Functions in records/lists remain ordinary functions with no hidden receiver                      |
| L07 | Direct, mutual, and indirect tail calls do not grow continuation depth                            |
| L08 | Tail-call temporaries and unreachable shadowed REPL bindings are collectible; live heap plateaus  |
| L09 | Overflow, bad conversions, non-finite floats, and invalid indexing are language errors            |
| L10 | Unit, Null, Option.None, empty List, and empty Stream remain distinct                             |
| L11 | attempt catches language errors but cannot swallow cancellation                                   |
| L12 | Script and REPL evaluation semantics agree apart from boundary display/recovery                   |
| L13 | Module caching preserves nominal identity; cycles and duplicate declarations fail                 |
| L14 | Failed REPL entries publish no partial bindings; earlier external effects remain                  |
| L15 | A closure captures resolved free bindings; an unrelated local Stream does not prevent publication |
| L16 | Session effects in callbacks follow ordinary call order; existing job snapshots stay unchanged    |

Minimum tail tests perform one million tail calls under debug, ASan/UBSan, and release
profiles. Check both continuation depth and live heap after collection; an RSS-only test
can confuse allocator retention with actual live objects. Non-tail recursion must fail
by an evaluator limit rather than C stack corruption.

Syntax coverage includes closure/Record headers, field shorthand, atomic parameter
patterns, tight indexing versus List arguments, negative arguments, multiline
application, and statement boundaries inside nested closure bodies. Legacy calls are
rejected. Wide Record tests check field order, duplicate keys, embedded NUL, and
allocation failure in validation scratch. Retained closures test destructured parameter
shadowing after their defining bindings are replaced. Process tests distinguish staged
closure effects from delayed multi-parameter bodies and prove that structural pattern errors prevent all entry effects. PTY coverage
checks closure continuation, bare function submission, and recovery after malformed calls.

### Closures and library composition

Exercise recursive expressions after partial application and scope shadowing, independent
factory captures, retained nested closures, and million-call tail recursion. Export tests
cover empty/omitted tables, aliases, qualified re-exports, shared standard ADT identities,
private bootstrap bindings, and rejection of removed export syntax before effects.
Command dispatch remains hygienic inside closures even when a local `run` is a Stream;
that unrelated resource must not enter the closure's captures.

Sequence tests check callback order, demand-only unfolding, short-circuit cleanup of
infinite producers, malformed callback outcomes, and retained producer failures. Standard
input tests cover exact binary transport, exclusive ownership, release after close,
command-input conflicts, and script-source EOF. Pure helpers run under stress GC;
numeric parsing distinguishes malformed tokens, embedded NUL, and numeric overflow.
Allocation-failure sweeps cover builders, recursive captures, paths, and cutoff.

## ADTs, records, and patterns

| ID  | Contract                                                                             |
| --- | ------------------------------------------------------------------------------------ |
| A01 | Nominal identity cannot be forged by record keys or equal display names              |
| A02 | Constructors are first-class; a fieldless constructor is a value, not a thunk        |
| A03 | Construction rejects missing/extra/duplicate fields independently of key order       |
| A04 | Exact/open/rest patterns have the specified field and length behavior                |
| A05 | Anonymous patterns do not implicitly match nominal products                          |
| A06 | Parameter patterns are checked at each curried application                           |
| A07 | Match subject evaluates once; branches/guards preserve source order                  |
| A08 | Guard errors propagate; false guards do not leak bindings or roll back effects       |
| A09 | Invalid pattern structure is rejected before entry effects, including in unused code |
| A10 | `with` preserves identity and rejects insertion; base values remain unchanged        |
| A11 | Equality rejects nested unsupported values regardless of field traversal order       |
| A12 | List-tail matching avoids quadratic copying and survives GC                          |

Generated tests can check record equality under field permutation, constructor
projection, update immutability, and parsing/printing round trips for the explicitly
round-trippable subset. Functions and resources are not in that subset.

## Processes, streams, and resource safety

| ID  | Contract                                                                                                           |
| --- | ------------------------------------------------------------------------------------------------------------------ |
| E01 | Plan construction captures arguments once and opens no redirection files                                           |
| E02 | Separate launches use distinct Jobs; explicit and ambient cwd/env rules hold                                       |
| E03 | Empty, spaced, newline, wildcard, leading-dash, and invalid-UTF-8 path arguments survive                           |
| E04 | NUL is rejected at argv/env/path boundaries with the responsible argument identified                               |
| E05 | ENOEXEC does not invoke another shell; actual exit 127 differs from launch failure                                 |
| E06 | Redirection order and overridden pipeline endpoints behave exactly as specified                                    |
| E07 | Large concurrent stdin/stdout/stderr traffic cannot deadlock                                                       |
| E08 | A pipeline larger than kernel pipe capacity starts all necessary stages before waiting                             |
| E09 | Expected downstream cutoff differs from failure and user cancellation                                              |
| E10 | Byte EOF followed by unsuccessful process exit still fails a checked stream                                        |
| E11 | Transfer or close invalidates old stream aliases; reentrant consumption fails                                      |
| E12 | Scoped Stream handles in records and closures are rejected before persistent publication; Job handles remain valid |
| E13 | Capture/materialization limits fail clearly and reap producers without truncating success                          |
| E14 | Partial launch and codec failures close all descriptors and reap every owned child                                 |
| E15 | Closed standard descriptors and dup2 self-mappings do not leak CLOEXEC mistakes                                    |
| E16 | Child signal masks/dispositions are reset; inherited ignored SIGPIPE does not persist                              |
| E17 | Stopped mixed contexts resume in foreground; bg rejects them without losing state                                  |
| E18 | Cleanup escalates and reaps owned children without signaling groups after ownership ends                           |
| E19 | Inherited open-file-description flags remain unchanged, including when FDs are duplicated                          |
| E20 | Cancelled reports fail check even when every stage exits zero; wait returns cancellation as report data            |

Run helpers that emit substantially more than typical pipe capacity on both stdout and
stderr while reading stdin. Compare exact bytes and stage reports. Send Ctrl-C during
every stage of launch and draining. Force the third stage's exec/redirection to fail
while the first two are active. Repeatedly launch short commands and inspect both
internal ownership counters and OS-visible child/FD state where available.

Inspect inherited descriptor flags with `fcntl`. Test launch gates with immediately
exiting process-group leaders and children killed before exec. EOF on the error channel
alone must not be treated as proof of successful exec. Use process-group timeouts to
expose deadlocks instead of indefinite waits.

## Editor, Unicode, and PTY interaction

Except for implemented segmentation and terminal ownership, this section defines
stage-4 acceptance work. Test the pure editor components independently of a real terminal. A simple flat string
reference model is appropriate for gap-buffer edits and undo; it is a test oracle, not a
second production editor. Feed input in every partition for short cases and randomized
partitions for larger cases, holding byte-arrival times and deadline events fixed.
Compare final text, cursor, events, pending decoder state, and resource bounds.

| ID  | Contract                                                                                                                       |
| --- | ------------------------------------------------------------------------------------------------------------------------------ |
| I01 | Editing and undo preserve valid UTF-8 and grapheme cursor boundaries, including insertions that join neighboring clusters      |
| I02 | Segmentation passes every official Unicode 18.0.0 GraphemeBreakTest case; generated tables reproduce from pinned inputs        |
| I03 | Width fixtures cover combining-only clusters, CJK, variation selectors, emoji modifiers, flags, ZWJ sequences, and tabs        |
| I04 | Incremental UTF-8, CSI/SS3, and ESC-prefix decoding is independent of read chunk boundaries and respects deadlines             |
| I05 | Bracketed paste is one bounded transaction; newlines, control bytes, and command-looking text never submit or invoke shortcuts |
| I06 | Invalid/oversized/interrupted paste drains through the closing marker; its suffix never becomes a command                      |
| I07 | Undo/redo, multiline history, menu acceptance, and parser-driven submission preserve the documented editing state              |
| I08 | Layout preserves graphemes across wrapping, viewport movement, resize, and the right margin                                    |
| I09 | Partial output writes cannot interleave escape sequences; redraw coalescing retains the unsent suffix                          |
| I10 | Highlighting performs no I/O/evaluation; cursor movement or cancellation invalidates completion even without text changes      |
| I11 | Terminal entry/exit, foreground handoff, suspension, errors, and repeated resume restore the correct modes                     |
| I12 | History locking and replacement preserve complete entries across simultaneous sessions and interrupted writes                  |

Grapheme tests establish boundary conformance; they do not establish terminal-cell
width. Width is a separate project policy with deterministic fixtures and manual checks
in supported terminal emulators. Do not turn one emulator's glyph rendering into a
Unicode conformance claim.

PTY tests create a session and controlling terminal, then exercise these contracts
across editing, external execution, streaming, and pure evaluation. Include a child that
changes termios, terminal reads, TOSTOP, partial-key deadlines, resize during paste, and
background notifications. Verify restoration after errors and stop/resume. A hung
completion worker must not block editing or permit additional unreaped workers. A stream
renderer must display available items without requesting a layout sample.

Use terminal-semantic assertions and captured screen states rather than requiring one
equivalent ANSI encoding. Keep manual emulator checks separate from automated PTY
results. Actual cancellation/reaping tests use controllable children; do not interpret
one timeout measurement as a universal kernel-termination guarantee.

## Platform conformance

| ID  | Contract                                                                                                                                |
| --- | --------------------------------------------------------------------------------------------------------------------------------------- |
| P01 | PATH absent, empty, relative components, slash bypass, EACCES, and ENOEXEC follow the launch policy                                     |
| P02 | Child argv and environment preserve bytes; duplicate/malformed imported names follow the explicit import rule                           |
| P03 | cd commits physical PWD/OLDPWD or rolls back; rollback failure reports actual state and invalidates PWD                                 |
| P04 | XDG unset, empty, relative, absolute, and missing-HOME cases never fall back to the cwd                                                 |
| P05 | State creation uses private modes; unwritable history leaves a usable prompt; unused directories are not created                        |
| P06 | C, available UTF-8, and comma-decimal locale environments do not change language numbers, JSON, or sorting                              |
| P07 | Child locale variables remain unchanged; malformed UTF-8 text is rejected and non-UTF-8 Path/Bytes round-trip                           |
| P08 | TERM missing, dumb, unknown, recognized, and console profiles select the documented rich/plain behavior                                 |
| P09 | Color precedence covers empty/nonempty NO_COLOR, explicit CLI modes, RGB hints, and redirected output                                   |
| P10 | Ctrl-Z at the prompt restores the terminal and resumes the same buffer; Ctrl-_ remains undo                                             |
| P11 | Incomplete escape sequences, slow paste, resize, and job notifications do not indefinitely block supervision                            |
| P12 | Grapheme editing is locale-independent; all renderers share one width policy; malformed source and arbitrary path bytes remain distinct |
| P13 | Noninteractive launch does not seize the terminal or initialize editor/history; explicit -- and shebang invocation work                 |
| P14 | Editor initialization leaves supervisor and sanitizer handlers intact; prompt writes do not leak into redirected stdout                 |

## Fuzzing and failure injection

Three independent libFuzzer targets cover existing boundaries:

| Target        | Input and oracle                                                                                                                                                                                                      |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `json-fuzz`   | Raw document bytes; strict decode errors, supported-value encode/decode equality, and stress GC                                                                                                                       |
| `lines-fuzz`  | At most 128 KiB of bytes split at the midpoint and an input-derived offset into up to three nonempty chunks; line count/content against a byte scan, CRLF trimming, final fragments, UTF-8 errors, and stream cleanup |
| `syntax-fuzz` | Raw source bytes; validation, parse diagnostics, code preparation after source release, and abort/collection                                                                                                          |

JSON uses the adapter directly. Lines executes a fixed private-adapter expression,
without loading the prelude. Syntax never executes the input. Every invocation owns and
releases its heap; inputs of at most 1 KiB enable stress GC. These targets neither
launch processes nor access user files. The line oracle shares the production UTF-8
validator; independent Python codec tests supply known encodings, malformed sequences,
and expected line values across every two-part split of short fixtures and one-byte
chunks. Empty input has its own seed, distinct from an empty line. JSON system tests
check byte-budget boundaries and compare emitted data against Python values,
including numeric types and signed zero, rather than relying only on same-adapter round
trips.

Line partitions depend only on input bytes, retaining the entire payload. Mutations
therefore explore both content and chunk boundaries without introducing random state
between invocations.

```sh
CC=clang meson setup build/fuzz --buildtype=debugoptimized \
  --wrap-mode=nodownload -Db_sanitize=address,undefined,fuzzer-no-link -Dfuzz=true
meson test -C build/fuzz --suite fuzz --print-errorlogs
mkdir -p build/fuzz/json-corpus
build/fuzz/tests/fuzz/json-fuzz build/fuzz/json-corpus tests/fuzz/corpus/json \
  -max_total_time=60 -max_len=131072 -timeout=10 -artifact_prefix=build/fuzz/
```

Use `lines` or `syntax` in place of `json` for the other campaigns. Meson's optional
`fuzz` suite replays explicit seed files once using libFuzzer's native regression mode;
it does not mutate corpora. Mutation campaigns are separate from the correctness gate.
The first corpus directory receives discoveries; tracked seed directories are read-only
inputs. `.gitattributes` preserves seed bytes, including CRLF. Keep generated corpora,
crash artifacts, and logs in ignored build output. Reduce a finding with
`-minimize_crash=1`, then retain a minimal regression with a behavioral test where
useful.

Meson's sanitizer option instruments all compiled components and yyjson; only fuzz
executables link the driver. Use the matching Clang runtime and verify LeakSanitizer
support locally. Record duration, executions, limits, and findings per target; execution
counts from different harnesses are not comparable coverage measures. The [libFuzzer
manual](https://llvm.org/docs/LibFuzzer.html) defines corpus replay, merging, and crash
minimization. Editor input/layout/undo, argument encoding, and bounded pure evaluation
remain future targets. Do not execute arbitrary fuzz-generated effects.

Extend the [existing failure scenarios](#fixtures-and-failure-paths) at allocator and
POSIX boundaries as new paths are added. Verify unwind behavior under stress GC;
avoid a second implementation hidden in a generic syscall-mocking framework.

Property checks cover chunk boundaries, argument round trips, stable sorting with one
key evaluation per element, and JSON round trips for the supported finite-value subset.
Explicitly test duplicate JSON keys, numeric range limits, NUL strings, deeply nested
documents, and error paths through unsupported nested values.

## Performance evidence

Run release benchmarks with recorded source revision, compiler/options, architecture,
OS, and dataset; record the image digest when using a container. Keep machine details in
ignored local logs. Published evidence must identify both source revisions or content
hashes, never only an unnamed working-tree snapshot. Measure startup after any container
has started, and report distributions rather than a single best run.

The `tests/benchmarks/*.rill` workloads cover direct tail calls, curried calls,
structural equality, text comparison, `map`/`sum` materialization, closure construction,
shared DAGs, large Records, immutable Record updates, checkpoint unwinding, and
demand-driven source/map/filter/fold callbacks, destructured/Unit parameters, and
repeated wide aggregate construction, wide capture reads, and CRLF line decoding
with both complete and split chunks. Each
checks its result and launches no external programs. Text comparison constructs equal
and unequal Strings with a 1,024-byte common prefix at runtime; identical pooled
literals would mostly measure identity checks. C benchmarks isolate syntax/code
acquisition, explicit collection, binding snapshots, retained captures, differently
shared DAGs, scoped reachability, function-layout preparation, nested capture analysis,
repeated constants, and wide/deep JSON decoding and encoding. Run benchmarks separately
from the correctness gate; [status](status.md#performance-evidence) records the current count:

```sh
meson test -C build/release --benchmark --repeat=7 --print-errorlogs
```

Benchmark suites are `language`, `runtime`, `codec`, and `preparation`. They verify
results but impose no timing threshold or per-round heap-equality requirement. Retained
bytes are observations, not an allocator strategy contract; lifecycle regressions belong
to the component tests. Timing helpers only read the monotonic clock; Meson owns runs
and result logs. Its native `benchmark()` runs serially and omits randomized
`MALLOC_PERTURB_` injection. See the [Meson
reference](https://mesonbuild.com/Reference-manual_functions.html#benchmark).

C benchmark binaries are excluded from the default build; `meson test --benchmark`
builds them on demand. For before/after comparisons, retain both release executables,
warm each once, and alternate their execution order on identical scripts. Report median
and range, including shell startup and prelude loading. Keep raw timing logs in ignored
build directories. Do not mix release results with sanitizer timings or infer allocator
traffic or cache misses from elapsed time. Keep an unchanged workload and identical
compiler options when comparing two source snapshots. C-file introductions explain each
measurement boundary; the workload source owns sizes and iteration counts.

| Workload family                | Measurement boundary                                                                                                                             |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Syntax                         | Parse-only timing for wide statement sequences, wide Records, and long literals; prepare, execute, and validate outside the interval             |
| Bindings, snapshots, functions | Meson process duration, including work and validation; snapshots change the published value on every iteration                                   |
| GC and captures                | Explicit collection timed separately from graph construction; validate the surviving graph after the measured collections                        |
| DAG equality                   | Time comparison of differently shaped DAGs with equal unfoldings, excluding construction and validation                                          |
| Resource escape                | Scoped-handle traversal with an unrelated live Stream, preventing a trivial fast path                                                            |
| Preparation                    | Time inside `rill_runtime_begin`, including disposal of the preceding entry; validate the retained body afterward                                |
| JSON                           | Wide and deep decoding and encoding measured separately; input preparation, result validation, and explicit collection stay outside the interval |
| Rill scripts                   | End-to-end evaluation, including startup and prelude loading; no external commands                                                               |

Timing supplements lifecycle tests. Rooted builders, graph equality, closure/code
lifetimes, checkpoint unwinding, and immutable updates must remain correct under stress
GC and allocation failure regardless of benchmark results. Observed collection pauses
are not worst-case latency bounds.

Timed component intervals report iteration or collection counts with their totals;
divide by that count before comparing per-operation costs. Syntax, JSON, and preparation
also report input bytes. Stream scheduling has a deterministic contract test: when stream
progress permits evaluator work, it must not request an I/O wait. The stream benchmark
measures throughput without imposing a wall-clock assertion on that contract.

Initial engineering budgets are less than 16 ms for ordinary local-buffer highlighting
and expiration of a pending completion request at its 200 ms deadline. Completion never
blocks editing. These are planned engineering targets to measure on a declared test
configuration, not guarantees about every host or kernel scheduling latency. Measure
pipeline throughput, bounded queue behavior, allocation per row, retained heap after
repeated REPL entries, and explicit collection/sorting costs. Keep functional
correctness independent of noisy wall-clock benchmark thresholds.

## Delivery gates

Formatting, static analysis, generated-data verification, and documentation checks
follow [development](development.md). The `api-docs` target must succeed without
warnings; review rendered C23 declarations and ownership contracts as well as diagnostic
output. Check Markdown links, anchors, tables, and examples, and exclude personal paths
from published artifacts. These are development checks, separate from runtime contract
IDs.

The [implementation plan](implementation-plan.md) owns the delivery stages, including
the pending stage-3.5 stream extensions, and maps them to these contracts. Each stage
includes its relevant tests and failure paths;
subsequent gates retain the earlier regressions. Until a feature is complete, record
partial/pending contract coverage explicitly rather than treating absent cases as
passes. The final gate requires the complete platform matrix and all contracts above.
