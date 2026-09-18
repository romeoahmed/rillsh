# Implementation status

Stages 1–3 are complete. Rill Shell combines external pipelines with its functional
language, reusable job plans, scoped structured streams, filesystem/JSON adapters, and
resumable foreground evaluations. The [implementation plan](implementation-plan.md)
places resource-owning producers, multi-input composition, and a measured fusion decision
in stage 3.5, before rich editing, completion, persistent history, and delivery in stage 4. The specifications describe the full first-release target; this page separates
implemented behavior from that remaining scope.

## Available surface

The executable accepts `-c SOURCE`, a source file with arguments, redirected stdin, or
interactive canonical input. Complete entries are parsed before evaluation; incomplete
entries continue at the prompt. Invocation supports `--`, `-i`, `--help`, `--version`,
color overrides, and `--no-config`. Interactive startup loads XDG configuration; scripts
receive argument Bytes through `args ()`.

The language provides checked arithmetic, distinct String/Bytes/Path values, immutable
Lists and Records, first-class curried closures, proper tail calls, nominal structs and
enums, nested patterns and guards, immutable updates, and explicit errors. Whitespace
application, ordinary and recursive closures, explicit export tables, Record shorthand,
and `match ... of` share one grammar. Structural pattern errors are rejected before entry
effects. Closures retain resolved free bindings. Failed entries publish no bindings
but preserve completed effects. Imports cache file identity, preserve nominal identity
across aliases, reject cycles, and permit retry after failure. Relative imports use the
importing directory.

The prelude and the core, sequence, text, filesystem, process, JSON, Option, and Result
modules are embedded through C23 `#embed`. Modules own their implementations; the
prelude selects their exports. Composition and defaults use ordinary Rill functions and
pure partial applications over private primitives. The installed shell needs no library
source files or Python.

| Surface            | Implemented behavior                                                                                                                                                                 |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Commands and plans | Literal/quoted words, scalar/list-spread substitutions, byte pipelines, ordered redirection, reusable and programmatic plans                                                         |
| Job policies       | Launch snapshots, explicit cwd/environment overrides, accepted exit codes, stage reports, expected cutoff, cancellation                                                              |
| Process APIs       | `run`, `execute`, `start`, `wait`, `check`, `capture`, `stream`, `through`, `fg`, `bg`, `cancel`, `jobs`                                                                             |
| Streams            | Single-consumer ownership, lazy `map`/`filter`/`filter_map`/`take`/`drop`, `items`, `unfold`, `range`, stdin, explicit close, checked producer completion, bounded byte queues       |
| Consumers          | `collect`, `collect_bytes`, `fold`, `fold_until`, `find`, `any`, `all`, `each`, `sum`, stable `sort_by`, `write_stdout`; explicit materialization limits                             |
| Text and JSON      | Explicit byte/text output, split/join/trim, decimal parsing, UTF-8 line decoding across chunks; strict document conversion with duplicate-key, numeric-range, depth, and byte checks |
| Filesystem         | Lexical path composition/decomposition, directory records, byte-sorted explicit globbing, bounded regular-file text reads, safe Path display                                         |
| Session            | Explicit cwd/environment effects; suspended pure/mixed evaluations, foreground resumption, shutdown cleanup                                                                          |

Returned streams display incrementally at the prompt. Text and path bytes are escaped;
Lists, Records, and plans display counts with units. Unicode-aware tables belong to
stage 4. The prompt uses canonical terminal editing. RGB, 256-color, 16-color, and plain styles are
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

## Pending stream capabilities

[Stage 3.5](implementation-plan.md#35-extensible-stream-production-and-composition)
is planned. `unfold` already supports demand-driven user code with explicit state, but
there is no custom acquisition/release protocol, `zip`, `merge`, or automatic stream
fusion. Resource-owning producers and multi-input composition are planned deliverables;
fusion is subject to semantic verification and measured benefit. Existing pipeline
composition does not imply any of these missing capabilities.

## Acceptance evidence

There are 32 ordinary Meson tests and three optional fuzz seed-replay tests. C component
and fault tests and Python script/process/PTY tests exercise the production
implementation. Counts describe the current suite, not coverage percentages.
[Testing](testing.md) owns the detailed contracts and reproduction commands.

| Contracts             | Evidence                                                                                                                                 |
| --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| L01–L04               | Syntax matrices, unary application, value-pipe evaluation order                                                                          |
| L05–L08               | Lexical captures, retained closures, million-call tail scenarios, stress GC, retained-heap plateau checks                                |
| L09–L12               | Checked arithmetic/conversions, distinct empty values, Result conversion, script/REPL boundaries, uncaught cancellation                  |
| L13–L16               | Module identity/cycles/retry, atomic publication, exact captures around local Streams, callback/session effect order                     |
| A01–A12               | Nominal identity, first-class constructors, patterns and guards, immutable updates, equality, shared List tails                          |
| E01–E08               | Plan reuse/snapshots, argument bytes, redirections, launch distinction, large concurrent stdin/stdout/stderr transport                   |
| E09–E14               | Cutoff, failure after byte EOF, alias/reentrancy rejection, resource escape, limits, checkpoint cleanup, failed launches/codecs          |
| E15–E20               | Descriptor/signal contracts, pure and mixed stop/resume, multiple contexts, cancellation, rejected background execution, forced shutdown |
| P01–P04, P07, P13–P14 | PATH/environment, directory transactions, XDG startup, UTF-8 boundaries, invocation, terminal ownership                                  |
| P08–P09               | Color selection and canonical-input fallback; rich-terminal selection remains stage 4                                                    |
| I02                   | All 853 official grapheme cases and deterministic regeneration; no full display-width claim                                              |

Stream regressions include no callback lookahead after `take`, nested consumers,
demand-only generators, short-circuit folds, stdin suspension/lease cleanup,
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

The 2026-09-19 testing and tooling audit passed **32/32** ordinary Meson tests in each profile
below. Versions identify tested configurations, not requirements.

| Platform             | Profiles                                    |
| -------------------- | ------------------------------------------- |
| Linux/glibc, AArch64 | Clang 23.1.0 ASan/UBSan; GCC 16.2.1 release |
| macOS, arm64         | Clang 23.1.1 ASan/UBSan                     |

The macOS build also passed clang-format, clang-tidy, and Doxygen.
System coverage includes language effects, process invocation, stream ownership,
independent codec oracles, and real PTYs. Meson formatting passed. Linux sanitizer runs
include leak checking. Python formatting, lint, and type checks passed. Regenerating
Unicode tables produced identical bytes. Six generator unit tests cover property
parsing, range normalization and conflicts, input hashes, and byte-exact CLI checks,
including newline drift and read-only failure behavior.

The three optional fuzz seed tests were rerun on macOS, including recursive
closures, export tables, empty input, split CRLF, and malformed UTF-8. The latest
macOS ASan/UBSan mutation campaigns used a 128 KiB input limit and finished without
a finding:

| Target             | Date       | Executions | Reported duration |
| ------------------ | ---------- | ---------: | ----------------: |
| JSON               | 2026-09-19 |  3,057,027 |              46 s |
| Lines              | 2026-09-19 |    155,861 |              46 s |
| Syntax/preparation | 2026-09-19 |  3,382,592 |              46 s |

These are bounded mutation runs, not exhaustive coverage or comparable throughput
measurements. Corpora and raw logs remain local build artifacts.

Stress-GC and fault regressions cover partial builders, exact captures, recursive
factories, ordered captures across closure instances, pooled literals, Record indexes,
suspended publication over newer bindings, checkpoint cleanup, and failed-entry recovery.
Abort clears borrowed diagnostics before collection and permits subsequent definitions.
Generated tables and dependencies are not hand-edited.

Remaining delivery checks include x86-64, macOS LeakSanitizer, rich-editor emulators,
and the planned editor and bounded-evaluator fuzz targets. Stage-4 contracts are not
inferred from passing stream or language tests.

## Performance evidence

All 32 release benchmarks pass on macOS/Clang and Linux/GCC. They validate workloads
without timing thresholds. On the tested arm64 macOS ABI, Values occupy 16 bytes, object
headers 64 bytes, and syntax nodes 80 bytes; these are observations, not ABI promises.
Prepared capture slots reuse node payload storage without increasing these sizes.

The 2026-09-19 comparison used macOS arm64, Clang 23.1.1, and Meson release builds with
identical benchmark harnesses for both builds. This comparison predates the later
validation refinements and JSON encoding benchmarks. Each binary was warmed once,
followed by seven measured rounds alternating before/after order (31 rounds for startup). No build, test,
or fuzz workload ran concurrently. Parser, preparation, and collection totals are
divided by their iteration counts; other measurements include process startup and
validation. Times below are milliseconds: median [minimum, maximum].

| Workload                         |                        Before |                         After |
| -------------------------------- | ----------------------------: | ----------------------------: |
| Wide capture reads               | 118.7683 [114.4961, 122.3972] |    85.2284 [83.2373, 93.8725] |
| Curried calls                    | 301.7642 [289.7500, 310.0159] | 290.0186 [277.0864, 301.5748] |
| Tail calls                       | 467.6129 [451.0069, 482.4221] | 468.3181 [441.1068, 480.3525] |
| Destructured/Unit parameters     | 130.9439 [127.4455, 161.9925] | 127.2564 [122.5193, 129.5285] |
| Binding snapshots                |    69.6932 [68.9952, 70.5871] |    67.5872 [66.6026, 69.1614] |
| CRLF line decoding               | 410.4447 [401.5550, 425.1055] | 415.4341 [397.5395, 422.4523] |
| Stream callbacks                 | 298.5685 [290.1114, 316.3079] | 304.0077 [295.3064, 320.5486] |
| Parse 4,096-field Record         |       0.6603 [0.6515, 0.6746] |       0.6552 [0.6513, 0.6781] |
| Prepare nested functions         |       0.0342 [0.0325, 0.0357] |       0.0344 [0.0333, 0.0372] |
| Prepare distinct literals        |       0.2647 [0.2633, 0.2678] |       0.2667 [0.2629, 0.2753] |
| Collect live DAG and dead leaves |       0.8661 [0.8640, 0.8831] |       0.8671 [0.8587, 0.8783] |
| Startup with Unit entry          |       2.5518 [2.3940, 3.1586] |       2.5435 [2.2025, 3.2627] |

Prepared lexical slots remove repeated name comparisons for captured references. The
wide-capture workload's median falls by about 28%, with nonoverlapping observed ranges.
Publication merges pending bindings directly into the current snapshot, removing an
intermediate GC binding chain. Its roughly 3% median improvement is smaller, with
partially overlapping ranges. These comparisons measure the combined change set;
they do not isolate each optimization's contribution.

Bounded identifier copies remove unused growable-buffer capacity from retained syntax.
Charged storage falls from 1,454,115 to 1,200,055 bytes in the 2,048-function workload
(about 17%), and from 654,900 to 413,061 bytes in nested preparation (about 37%). Charged
storage excludes allocator overhead and is not RSS. Small capture layouts assign slots
during free-name discovery; only wider sorted layouts need an additional body walk.
Nested preparation remains effectively unchanged in this comparison.

The line decoder avoids staging complete in-chunk lines, but its end-to-end median
increases by about 1%, with overlapping ranges; callback streams also show no stable
speedup. Tail calls, startup, and collection show no material improvement. No cache-miss
reduction, bounded GC pause, or general interpreter speedup is established. The
collector, object header, allocator, and prepared-AST execution model remain unchanged.

Source identities are SHA-256 hashes over every file under `src/` and `stdlib/`, sorted
by relative POSIX path, hashing each path, NUL, file contents, and NUL in order:

- Before: `ce669fb8c7f78316f88ca30b6ba85149b1af0426fb7c93719523c3097a9b75a7`.
- After: `9f6a58d3706b0b51ff1ba882342c32c93c0cda4da0f3d092d7949d8ce5fd9c0f`.

Earlier measurements motivated sorted Record validation, bulk String decoding, nested
free-name summaries, constant sharing, and removal of an unnecessary I/O wait before
callbacks. They precede this baseline and are not attributed to this comparison.
[Architecture](architecture.md#optimization-policy) records the retained design and
tradeoffs; [testing](testing.md#performance-evidence) defines reproducible measurement
boundaries. Raw samples and experimental binaries remain local build artifacts.
