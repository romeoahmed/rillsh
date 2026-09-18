# Implementation status

Stages 1 and 2 are complete: Rill Shell executes external commands and provides its
functional language, reusable job plans, and bundled pure library. The language,
execution, interaction, and platform specifications describe the **first-release
target**; structured streams and the editor remain in stages 3 and 4 of the
[implementation plan](implementation-plan.md).

## Available surface

The executable accepts `-c SOURCE`, a source file with arguments, redirected stdin, or
interactive canonical input. Complete entries are parsed before evaluation; incomplete
entries continue at the prompt. `--`, `-i`, `--help`, `--version`, color overrides, and
`--no-config` are supported. Interactive startup loads the XDG configuration file;
scripts expose arguments as Bytes through `args()`.

The language includes checked Int/Float arithmetic, Bool, Null, Unit, String, Bytes,
Path, immutable Lists and Records, first-class curried closures, proper tail calls,
nominal structs/enums, nested patterns and guards, immutable updates, and explicit
errors with `attempt`. Closures retain only resolved free bindings. Later REPL shadowing
preserves earlier captures; a failed entry publishes no bindings while preserving
effects already performed.

Imports produce immutable export namespaces. File identity caching preserves nominal
identity across aliases, rejects cycles, and permits retrying failed loads. Relative
imports use the importing source directory, independently of later `cd` effects.
`std:core`, `std:seq`, `std:text`, and `std:process` are bundled through C23 `#embed`.
Their public operations and defaults use ordinary Rill functions over private
primitives. The installed executable needs neither library source files nor Python.

Commands support literal/quoted words, scalar and list-spread substitutions, byte
pipelines, and ordered redirection. `job { ... }`, `command`, and `pipe` construct
reusable plans without launching processes or opening redirection files. `with_cwd`,
`with_env`, and `accept_exit` attach explicit per-stage policies.

| Operations | Current behavior |
| --- | --- |
| `run`, `start` | Run a plan in the foreground or create an independent background job |
| `wait`, `check` | Inspect a nominal JobReport or raise its selected failure |
| `fg`, `bg`, `cancel` | Control external jobs; mixed suspended evaluations belong to stage 3 |
| `jobs` | Return records containing `id`, `handle`, `kind`, and named `state` |
| `cd`, `pwd` | Change directory transactionally; return the physical directory as Path |
| `get_env`, `set_env`, `unset_env` | Read and update the session environment explicitly |
| `exit`, `exit_force` | Refuse live jobs or explicitly clean them up before exit |

The prompt still uses canonical terminal editing. Rich editing, completion, persistent
history, and Unicode-aware value presentation are pending. Current display escapes
non-ASCII bytes and controls; RGB, 256-color, 16-color, and plain styles are available.
Pinned Unicode 18.0.0 grapheme segmentation is implemented; complete display-width
validation belongs to stage 4.

## Ownership and limits

- A precise, nonmoving collector uses explicit roots and iterative marking. Owned code,
  syntax storage, captures, and shared list-tail views participate in lifetime accounting.
  Unreachable recursive closures and shadowed bindings are collectible.
- Continuations are explicit; direct, mutual, and indirect tail calls replace frames.
  Non-tail evaluation is limited to 65,536 live continuations and syntax to 256 levels.
- Sorting evaluates each key once in source order, preserves ties, and checks item and
  retained-byte budgets. No structured Stream values exist yet.
- Each launch owns its argv/environment/cwd snapshot. A launch gate and separate error
  channel distinguish launch failure from a program that exits 127. Stage policies
  participate in rightmost-failure and expected-SIGPIPE handling.
- One cooperative loop handles signals, child state, bounded I/O work, and cancellation.
  Capture/feed pumps are tested through the C supervisor API; public streams and codecs
  remain stage 3 work.
- Cancellation cannot become success when children exit zero. TERM/CONT precedes timed
  KILL escalation and reaping. Interrupting `wait` leaves an independent job alive.
- Completed, acknowledged jobs are pruned when no reachable handle retains them.
  The supervisor permits 1,024 retained jobs and 256 stages per pipeline.
- The session owns the terminal through a separate open file description. Foreground
  handoff, stop/resume, and restoration preserve inherited descriptor flags.

## Acceptance evidence

The suite registers 29 Meson tests. C language tests exercise the evaluator directly;
Python tests exercise scripts, module files, real processes, and controlling PTYs.
Counts are a snapshot, not a coverage target. [Testing](testing.md) defines the
contracts.

| Contracts | Evidence and boundary |
| --- | --- |
| L01–L04 | Syntax matrices; application and value-pipe effect-order scripts |
| L05–L08 | Retained closures, exact captures, three million-call tail scenarios, continuation limits, stress GC, and retained-heap plateau checks |
| L09–L12 | Arithmetic/conversion/index failures, distinct data values, Result conversion, script/REPL behavior, and uncaught Ctrl-C; Stream branches pending |
| L13–L14 | Cached module identities/cycles, retry after failed import, nominal redeclaration, and atomic REPL publication with preserved effects |
| A01–A06 | Nominal identity, first-class constructors, payload validation, nested/exact/open/rest patterns, and curried parameter checks |
| A07–A12 | Subject/guard order, failed-guard scope, repeated names, immutable updates, equality validation, and shared list tails under GC |
| E01–E02 | Reusable/programmatic plans, prelaunch validation, argument spreading, per-stage policies, and distinct launch snapshots |
| E03–E08, E14–E16, E18–E20 | Retained command/supervisor regressions; direct feed/capture tests; codec and Stream branches pending |
| P01–P03, P07, P13–P14 | PATH/environment bytes, directory transactions, UTF-8 boundaries, invocation, signal and terminal ownership |
| P04, P08–P09 | XDG startup matrix and color selection; history storage and rich-terminal selection pending |
| I02 | All 853 official grapheme cases and deterministic regeneration; no full display-width claim |

Allocation-budget tests exercise production parser/runtime code, including closures,
ADTs, patterns, and equality. Single-failure library sweeps cover rooted builders and
error propagation after the allocator recovers. Other fault tests retain directory
rollback, partial launch, and children stopped or terminated before exec. Test aliases
stay outside production interfaces. Cleanup coverage is limited to owned children and
descriptors; it does not establish containment of descendants that leave the group.

Stream-dependent L10–L11 and L15–L16 cases, remaining E-series cases, and rich
interaction contracts remain pending. No editor, codec, or Stream coverage is implied by
a passing language suite.

## Validation checkpoint

The 2026-09-18 test checkpoint passed **29/29** Meson tests in each profile below.
Versions identify tested configurations, not compiler or platform version requirements.

| Platform | Profiles |
| --- | --- |
| Linux/glibc, AArch64 | Clang 23.1.0 ASan/UBSan and release; GCC 16.2.1 release |
| macOS, arm64 | Clang 23.1.1 ASan/UBSan and release |

Both complete quality gates passed formatting, clang-tidy, Doxygen, generated-data,
repository checks, and tests. Ruff lint/format and ty passed for Python tools and system
tests. The system suites contain 12 language, 19 process/invocation, and 14
controlling-PTY methods. Five repeated macOS release PTY suite runs passed. Separate
timeout probes verified cooperative exit and forced termination, with pipe closure and
child reaping.

Unoptimized syntax/runtime/language suites passed **12/12**, including one million calls
each for direct, mutual, and indirect tail recursion. Release benchmark result checks
passed **13/13**; these are correctness results, not speed thresholds. Linux leak
checking was exercised. x86-64, macOS LeakSanitizer, rich-editor emulator behavior, and
fuzz targets remain unverified or unimplemented.

Reproduce the gate and selected suites with the commands in
[development](development.md#quality-gate) and
[testing](testing.md#test-infrastructure). These results apply to that checkpoint; rerun
affected checks after changes.

The documentation review reran the complete macOS Clang ASan/UBSan gate (**29/29**),
Ruff, and ty. All four README examples passed through both command and file input.
Generated API pages include C23 attributes and the documented static limits. C token
comparison confirmed that the review changed comments, not executable C. Linux and
release profiles above were not rerun for this documentation-only review.

## Performance evidence

Current representations include flat captures, compact object headers, indexed Records,
packed binding snapshots, shared List slices, prepared constants, and graph-aware
equality. [Architecture](architecture.md) explains their ownership and cost models. The
13 retained benchmarks cover these paths; [testing](testing.md#performance-evidence)
defines the comparison procedure.

Earlier local measurements used intermediate working-tree snapshots without durable
revision identities. They do not establish a reproducible release comparison. No general
speedup, cache-locality improvement, or bounded GC pause is claimed. Future comparisons
must identify both revisions and retain identical workloads and build options.
