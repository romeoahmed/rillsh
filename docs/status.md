# Implementation status

Stage 1 is complete. This page describes the implemented subset; the language,
execution, interaction, and platform specifications define the **first-release target**.
The remaining milestones are in the [implementation plan](implementation-plan.md).

## Available surface

The executable accepts `-c SOURCE`, a source file, redirected stdin, or interactive
canonical input. The parser recognizes complete, incomplete, and invalid entries;
complete entries are parsed before evaluation. `--`, `-i`, `--help`, `--version`,
and color overrides are supported. `--no-config` is accepted but has no effect yet.
Script arguments are accepted but are not exposed through `args()` yet.

Implemented expressions include Unit, Int, UTF-8 String, List, indexing, field
projection, immutable `let`, and unary application of first-class native functions.
Bytes and Path use explicit constructors. There are no user closures, arithmetic,
record literals, nominal types, pattern matching, or modules yet.

Commands support literal/quoted words, scalar `$name` and `$(expression)` arguments,
byte pipelines, `<`, `>`, `>>`, `2>`, `2>>`, and `2>&1`. `job { ... }` creates a
reusable plan; list spreading and programmatic plan composition are still pending.

| Native operations    | Current behavior                                                        |
| -------------------- | ----------------------------------------------------------------------- |
| `run`, `start`       | Execute a plan in the foreground or start a background job              |
| `wait`, `check`      | Return an opaque report; check its aggregate completion status          |
| `fg`, `bg`, `cancel` | Control external jobs; suspended language continuations are pending     |
| `jobs`               | Return records with `id`, `handle`, and a numeric `state`               |
| `cd`, `pwd`          | Change directory transactionally; return the physical directory as Path |
| `bytes`, `path`      | Construct byte data and NUL-free paths                                  |
| `exit`, `exit_force` | Refuse live jobs or explicitly clean them up before exit                |

`jobs()[0].handle` recovers a job handle. Reports do not yet expose the nominal
completion/stage data in the execution specification. The prompt uses canonical
terminal editing; custom key bindings, history, completion, and raw-mode editing are
not implemented. Display escapes non-ASCII bytes and controls; Unicode-aware value
presentation and the complete emoji-width policy remain pending.

## Foundation and ownership

- A precise, nonmoving collector uses explicit roots and iterative marking. The
  evaluator uses continuation frames and publishes entry bindings only on success.
- Each launch copies argv, environment, cwd, and feed data. Ordered redirections,
  close-on-exec descriptors, a process-group launch gate, and a separate error channel
  distinguish launch failure from a program that exits 127.
- One cooperative loop handles signals, child state, bounded I/O work, and cancellation.
  Capture/feed pumps are tested through the C supervisor API; public structured streams
  and codecs are not implemented.
- Cancellation preserves its outcome even when children exit zero. TERM/CONT precedes
  timed KILL escalation and reaping. Interrupting `wait` leaves an independent job alive.
- Completed, acknowledged jobs are pruned when no reachable handle retains them.
  The supervisor permits 1,024 retained jobs and 256 stages per pipeline.
- The terminal has one owner and a separate open file description. Foreground handoff,
  stop/resume, and restoration preserve inherited descriptor flags.
- RGB, 256-color, 16-color, and plain styles are available. Unicode 18.0.0 grapheme
  segmentation uses pinned inputs; full terminal-width validation belongs to stage 4.

## Acceptance coverage

The existing suite registers 20 Meson tests, including 853 official grapheme cases,
17 Python process cases, and 9 controlling-PTY cases. Counts are a snapshot, not a
coverage target. [Testing](testing.md) defines the full contract set.

| Contracts                       | Evidence and boundary                                                                                                               |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| E03–E08, E14–E16, E18–E20       | Implemented command/process paths; direct supervisor tests for feed/capture; codec and Stream branches pending                      |
| P01–P03, P07, P13–P14           | PATH/environment bytes, directory transactions, UTF-8 boundaries, invocation, signal and terminal ownership                         |
| I02                             | Complete pinned grapheme corpus and deterministic regeneration; not full display-width conformance                                  |
| Language/interaction foundation | Unary natives, transactional publication, GC stress, multiline input, color, and external job control; full L/A/I contracts pending |

Fault tests compile selected production units with eight local allocator/POSIX aliases.
They cover allocation failures, directory rollback, partial launch, and children stopped
or terminated before exec. Production code contains no test hooks. Cleanup tests check
owned child and descriptor lifetimes; they do not claim exhaustive fault coverage or
containment of descendants that leave the process group.

The stage-1 validation checkpoint on 2026-09-18 recorded all 20 tests passing in:

| Platform           | Profiles                                                |
| ------------------ | ------------------------------------------------------- |
| Fedora 45, AArch64 | Clang 23.1.0 ASan/UBSan and release; GCC 16.2.1 release |
| macOS 27, arm64    | Clang 23.1.1 ASan/UBSan and release                     |

Formatting, clang-tidy, Doxygen, generated-data checks, and standalone boundary-header
compilation passed at that checkpoint. Leak checking was exercised on Fedora; macOS
LeakSanitizer and x86-64 were not validated. These are recorded results, not a substitute
for running the [quality gate](development.md#quality-gate) after subsequent changes.
