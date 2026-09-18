# Repository and implementation plan

This document maps the [architecture](architecture.md) to source ownership and
implementation stages, including stage 3.5 before interactive delivery.
[Language](language.md), [execution](execution.md),
[interaction](interaction.md), and [platform](platform.md) remain the behavior
contracts; [development](development.md) owns the toolchain and [testing](testing.md)
owns evidence. The layout describes the target repository. Add directories and
interfaces as their implementations become necessary.

## Repository layout

```text
meson.build                   Project, native build options, dependencies, install rules
.clang-format
.clang-tidy
.gitignore
src/
  meson.build                 Explicit source lists and private build targets
  main.c                      Executable entry; delegates CLI/session policy
  session/                    Composition, event loop, terminal ownership, REPL services
  source.c / source.h         Owned source, source identity, byte spans
  diagnostic.c / diagnostic.h Structured diagnostics; no direct printing
  text/                       Byte/text views, UTF-8, graphemes, display metrics, styled spans
    unicode_tables.inc        Checked-in generated properties; never hand-edited
  syntax/                     Source-level syntax and completeness
    syntax.h                  AST and parse-result contracts
    parse.c                   Token recognition, parsing, syntax ownership
  runtime/                    Values, GC, lexical scopes, continuations, resource tokens
    prepare.c                 Traced code ownership, constants, capture layouts
    eval.c                    Resumable prepared-AST evaluation
  exec/                       Process execution independent of language Values
    launch.c                  Launch snapshots and child setup
    supervisor.c              Job lifecycle, process groups, reports, byte pumps
  platform/                   Cohesive POSIX services and actual Linux/macOS differences
  editor/                     Planned: decoder, edits/undo, layout, rendering
  native/                     C primitives, native dispatch, codecs, standard-library bundle
    native.h / dispatch.c     Native-call boundary and effect dispatch
    bundle.c / bundle.h       Embedded std: module sources
stdlib/
  option.rill / result.rill   Ordinary ADT combinators
  prelude.rill
  core.rill
  seq.rill
  text.rill
  fs.rill                     Filesystem streams and explicit expansion
  process.rill
  json.rill                   Strict JSON conversion
subprojects/
  yyjson.wrap                 Pinned dependency; build overlay only if required
  packagefiles/               Any project-owned Meson overlay, separate from upstream source
data/unicode/                 Pinned input manifest, official properties/tests, license
tools/                        See the tools listed below
tests/                        See the test layout below
Containerfile                 Portable Fedora development image
docs/                         Specifications and this plan
  Doxyfile.in                 Minimal API-reference template; outputs stay in the build tree
```

`tools/` contains the Unicode generator/verifier, using Python's standard library. C23
`#embed` embeds the standard library directly. `tests/unit/` holds C component tests
and Python generator tests. `system/` holds Python process/PTY tests; `support/`
holds shared assertions, cleanup helpers, and the controlled child program.
`benchmarks/` holds release workloads; `fuzz/` holds syntax and codec targets
and seed inputs; keep fixtures with the cases that use them. Meson suite names remain
those in testing; directories and suites need not correspond one-to-one. Generated build
files and benchmark results stay under ignored build trees.

Keep headers beside their implementations. There is no installed C SDK or public
`include/` tree. `src/main.c` is the only production executable entry point. Use
lowercase C filenames with underscores between words; name implementations for their responsibility and
boundary headers for their component. `private.h` remains component-local. Test support
is shared infrastructure, not part of the unit-test subject. Benchmark names describe
the measured operation: `dag_equal`, `escape`, and `functions` distinguish graph equality,
resource-escape validation, and function preparation.

Keep tool-owned names such as `meson.build`, `meson.options`, `subprojects/`,
`packagefiles/`, `Doxyfile.in`, and Python's `test_*.py` discovery pattern. Keep `stdlib/`
for Rill modules and `src/native/` for their C implementation boundary.

## Module boundaries

The session assembles concrete components. Each component owns a domain and its
invariants; it does not receive the entire Session merely to obtain unrelated services.
Source and diagnostic types are small shared contracts, not the seed of a `utils` layer.
The following table lists allowed project dependencies; standard C facilities are
available everywhere, and source/diagnostic types are omitted from individual rows.
Editor dependencies describe planned work; see status for the implemented surface.

| Owner      | Responsibility                                                                                   | Direct component dependencies                         |
| ---------- | ------------------------------------------------------------------------------------------------ | ----------------------------------------------------- |
| `text`     | Length-aware text, Unicode properties/metrics, literal/style spans                               | None                                                  |
| `syntax`   | Syntax, completeness, precedence, AST, highlight/indentation metadata                            | `text`                                                |
| `runtime`  | Language values, code ownership, lexical resolution, GC, evaluation, resource tokens             | `syntax`, `text`                                      |
| `platform` | Native descriptor, terminal, signal, filesystem, environment, and clock services                 | `text` where byte/text views are needed               |
| `exec`     | Prepared launches, child identities, job state, reports, nonblocking byte transport              | `platform`, `text`                                    |
| `editor`   | Pure input/edit/layout state and rendering operations                                            | `syntax`, `text`                                      |
| `native`   | Callable primitives, codecs, filesystem/process adapters, library metadata                       | `runtime`, `exec`, `platform`, `text`; private yyjson |
| Session    | Scheduling, terminal handoff, startup, module I/O, history/completion coordination, presentation | Component interfaces above                            |

This is a dependency direction, not a requirement for one archive per directory.
`runtime` has no POSIX, editor, or yyjson dependency. `exec` accepts evaluated launch
data, never an AST or language Value. `editor` receives syntax results and copied
completion/help metadata, never an evaluator or a language closure to invoke. `platform`
contains mechanisms and native differences; it does not decide Rill job success, stream
escape, or REPL publication policy.

The small source/diagnostic implementations may depend on `text`, but never on syntax,
the runtime, or OS services. Diagnostic construction and diagnostic presentation are
separate: components return structured information; the session formats and emits it. A
value printer lives with the language adapter code and emits text/style spans, so the
editor need not know Value representation to display results.

### Interfaces that prevent cycles

**Language and processes.** The runtime owns the immutable logical JobPlan value and
opaque Job/Stream references. The library adapter translates an evaluated plan into an
`exec` launch specification with explicit byte strings, redirections, and policies.
Launch preparation produces the owned argv/env/cwd snapshot needed by children; it is
not a second persistent plan model. Returned status data becomes ordinary language
reports at the same adapter boundary. Exec objects retain no Values; language-side state
stays rooted in the runtime/adapter.

**Native calls and streams.** The runtime defines the native call/resumption protocol
and stream-token rules. Library code supplies native implementations, including source,
transform, and sink state. Implementations can request a language call, wait for I/O,
or complete; the session schedules progress. States retaining Values expose
explicit GC tracing/rooting through this internal protocol. OS cleanup is an explicit
scope operation, never a GC side effect. Keep these internal C interfaces limited to
their ownership and scheduling contracts.

**Session services.** Native registration receives the specific environment, job, and
resource services it uses. A suspended native operation retains owned state and a stable
registration token, not a pointer to a temporary C stack frame. A required callback
contract is declared by its consumer; the session supplies the implementation. No lower
component includes the session interface, re-enters the event loop, or polls a global
singleton.

**Terminal and editor.** The session owns the platform terminal handle and routes bytes,
deadlines, resize, and signal events into the editor. The editor returns edit actions,
submission outcomes, and rendering operations. The session writes them and arbitrates
job notifications and terminal handoff. History storage and completion-worker lifecycle
are session services; the editor owns their in-memory editing/search/menu state.

### File and header discipline

Start with a few files per component, splitting by a cohesive responsibility when
implementation size or tests justify it. For example, `runtime/value.c`, `heap.c`, and
`eval.c`, plus `native/stream.c` are useful boundaries; a separate file for each value
variant or evaluator opcode is not. `exec/launch.c` and `supervisor.c` separate launch
setup from live supervision; `editor/input.c`, `edit.c`, `layout.c`, and `render.c` separate
testable state machines. These examples are starting points, not an exhaustive file
checklist.

A component's boundary header exposes operations, immutable views, and explicit
ownership. Keep representation headers private; use forward declarations where useful,
not opaque heap objects for every small struct. Name external C symbols with a project
and component prefix, such as `rill_exec_`; keep helpers `static`. There is no umbrella
header that imports the whole application. Repeated policy belongs with its semantic
owner rather than in a catch-all `common`, `manager`, or `utils` module. Keep
`session/session.c` focused on orchestration; history persistence, completion-worker
coordination, and presentation get cohesive session-local files as they are implemented.

Boundary declarations carry concise Doxygen contracts under the [comment
conventions](development.md#comments-and-api-documentation). Document ownership and
failure behavior there; keep implementation invariants in ordinary local comments.

## Build, data, and library integration

The root Meson file owns project configuration; `src/meson.build` owns explicit sources
and component dependencies. Shared `files()` lists feed production archives and the
separately instrumented fault archive. `tests/meson.build` owns correctness tests;
`tests/benchmarks/meson.build` and `tests/fuzz/meson.build` own their respective targets.
`docs/meson.build` owns API-reference generation. Reuse private static targets between
the shell and tests.
Pure runtime/editor tests link without the supervisor or yyjson. Add targets and build
fragments for reuse or clear ownership, not one per directory. Meson dependency objects
carry include paths and link requirements.

Maintain one list of boundary headers for the optional `api-docs` target. The quality
gate requires it; ordinary shell builds do not. Cross-component includes must follow the
dependency table: include paths do not enforce encapsulation. Review guards component
boundaries and private-representation dependencies.

| Input                            | Output                            | Lifecycle                                             |
| -------------------------------- | --------------------------------- | ----------------------------------------------------- |
| Pinned Unicode data and manifest | `src/text/unicode_tables.inc`     | Explicit regeneration; both inputs and output tracked |
| Authored `stdlib/*.rill`         | Read-only bytes in the executable | Native C23 `#embed`; compiler-tracked dependencies    |

Bundling gives `std:` modules a deterministic source versioned with the executable.
Diagnostics use logical names such as `std:seq`; file imports retain their own identity
cache and relative-path rules. There is no installed-file search or cwd fallback for
bundled modules.

`src/native/` owns representation access, OS effects, and codecs; `stdlib/` expresses
composition and defaults in Rill. Bootstrap the private native registry, load bundled
modules, and install the prelude through the same unary application protocol. Builtin
help/signatures belong beside native registration, grammar metadata in `syntax`, and
Unicode properties in `text`. [Development](development.md) owns dependency acquisition,
generation checks, and tool configuration.

## Implementation stages

The stages are substantial integration milestones. Each delivers a runnable program
with acceptance evidence and extends the same parser, evaluator, supervisor, and event
loop. Commit size is independent of stage boundaries.

### 1. A reliable executable shell foundation

**Complete.** Build and toolchain, owned source/diagnostics, initial Values/GC, command
parsing, launch gates, byte pumps, cancellation, and external job control. CLI/script
input and a canonical prompt use the production parser and evaluator. Unicode generation
and RGB/plain output are available without rich editing.

Acceptance covers source-to-process pipelines, distinct syntax/launch/exit errors,
E03–E08, E14–E16, E18–E20, and P01–P03, P07, P13–P14 for this surface. PTY tests
establish external stop/resume and restoration. Linux and native macOS pass applicable
sanitizer/release gates, Unicode conformance, and warning-free API generation.

### 2. The complete functional language

**Complete.** First-class unary functions, currying, tail calls, records, ADTs,
patterns, checked arithmetic, errors, modules, lexical captures, transactional entry
publication, bundled libraries, and reusable process plans. The prompt remains plain.

Acceptance covers L01–L14 and A01–A12 for non-Stream cases, including million-call tail
recursion and stress GC; E01–E02 establish plan reuse and snapshots. Closures and
constructors survive module/entry boundaries. Failed entries preserve earlier effects
but publish no bindings. Stream-specific branches are accepted in stage 3.

### 3. Structured streaming and complete execution semantics

**Complete.** Scoped stream ownership, lazy transforms, checked sinks, materialization
budgets, filesystem/JSON adapters, and concurrent process transport. Foreground
suspension retains evaluator and resource contexts for resumption. Only one user context
runs at a time; rich editing, completion, and history remain stage 4.

Acceptance covers all L-, A-, and E-series contracts, including Stream branches, codec
boundaries, limit failures, cancellation, and cleanup before caught-error recovery. PTY
cases suspend pure and mixed evaluations, resume with `fg`, and reject unsupported `bg`
without losing state. Large simultaneous I/O remains bounded and producer failures
cannot disappear behind byte EOF.

[Current status](status.md) records the implementation and validation evidence for these
completed stages. Every stage retains the preceding regression suite.

### 3.5. Extensible stream production and composition

**Planned; not implemented.** Complete the remaining stream work before stage 4.
Stage 3 already provides `unfold`, `range`, short-circuit consumers, and scoped native
sources. `unfold` supports user-written state transitions; it does not provide a
user-defined acquisition/release protocol. Multi-input operators and automatic fusion
are absent. This stage adds lifecycle and composition capabilities, then evaluates
fusion against real Shell workloads.

**Deliver in dependency order:**

1. **Resource-owning producers.** Design a small library protocol for acquisition,
   demand-driven stepping, and release, building on `unfold` and the existing callback
   and cleanup machinery. Fix its signatures and outcomes in the execution specification
   before implementation. Specify when acquisition occurs, who cleans up failed
   acquisition, and how the latest producer state reaches release. Once acquisition
   succeeds, release must run exactly once on exhaustion, explicit close, cutoff,
   callback failure, cancellation, or scope exit. Define cleanup-error precedence and
   interruption behavior without allowing a release failure to skip other cleanup.
   Keep resources scope-owned and Values rooted across suspended callbacks; GC must
   never invoke language cleanup. Demonstrate a producer with observable acquisition
   and release, not only a numeric generator.
2. **Multi-input composition.** Implement a minimal `zip` and `merge` surface on the
   same scheduler. `zip` pairs items in input order and ends at the shorter input;
   specify pull order and any unavoidable final unmatched pull. `merge` preserves each
   input's order, serves ready inputs fairly, and finishes after all inputs end; its
   cross-input order is not deterministic. Validate all input tokens before transferring
   ownership, reject repeated or already-consumed inputs, and close every owned input
   on failure or downstream cutoff. Bound buffering per input, propagate backpressure,
   and retain observed producer errors. Pending I/O on one merge input must not stall
   another ready input. Do not run language callbacks concurrently or add nested event
   loops. Settle error precedence and cancellation semantics before adding the APIs.
3. **Evidence-driven stream fusion.** Measure adjacent lazy transformations on realistic
   text, filesystem, and process streams. Prototype only narrowly identified stream-node
   combinations; recognize operation identity rather than lexical spelling. Preserve
   callback order/count, staged application, errors, demand, cutoff, cancellation,
   checkpoints, and resource ownership. Eager List pipelines and arbitrary function
   applications are outside scope: fusing them can reorder effects. Retain fusion only
   with a repeatable benefit and acceptable implementation complexity; otherwise record
   the evidence and keep ordinary execution. A documented decision is required, not an
   unmeasured optimization or a claim that fusion exists.

**Boundary.** Keep coordination and ownership in the existing stream adapter and
session loop, with ordinary Rill wrappers in `std:seq`. Add runtime support only where
the existing callback protocol cannot express safe cleanup. This is not a general
coroutine, async/await, threading, broadcast/replay, or native-plugin framework. Public
producer signatures and multi-input argument shapes remain design work, not current
language syntax. Rich editing remains stage 4.

**Accept when:**

- Producer lifecycle tests cover acquisition failure, no demand, EOF, cutoff, nested
  callbacks, cleanup failure, Ctrl-C, and stop/resume under stress GC and allocation
  faults. Cleanup completes before recovery; resources cannot escape through state.
- Composition tests cover empty and unequal inputs, duplicate aliases, slow/blocked
  producers, simultaneous failures, bounded buffering, and early termination. PTY and
  process tests establish restoration, fairness, and cleanup on Linux and macOS.
- Any retained fusion passes differential effect/error traces against ordinary
  execution and records release measurements for throughput, allocation, retained
  memory, and startup. If rejected, the measured decision and absent capability remain
  explicit in status. Producer and composition delivery is not optional.
- Existing suites and the full quality gate pass. Update execution contracts, examples,
  test coverage, and status together; stage 4 inherits these acceptance obligations.

### 4. Complete interactive experience and delivery

**Deliver.** Implement the grapheme editor, incremental decoder, bounded paste/undo,
parser-aware submission/highlighting, layout/rendering, fixed key set, completion
worker, history persistence/search, and coordinated notifications. Integrate these into
the existing terminal owner and loop. Finish help, examples, API reference review,
installation, release workloads, fuzz regression corpora, and the complete Linux/macOS
validation matrix.

**Boundary.** Deliver the specified first release. Add no new language model, editor
mode framework, worker-thread runtime, terminal database, or compatibility backend
during completion of this stage.

**Accept when:**

- All existing acceptance contracts, stage-3.5 obligations, and named Meson suites pass
  on the required matrix; parser/editor/codec fuzz targets run under sanitizers, and examples
  work in their stated context. Documentation checks pass under the development policy.
  Unsupported checks are reported, not counted as passes.
- PTY tests prove paste isolation, grapheme edits, cursor-aware completion invalidation,
  concurrent-history integrity, notification redraw, and terminal restoration. Manual
  emulator checks cover the width/color behavior that a PTY alone cannot establish.
- A clean source distribution builds without network access, installs one working shell
  with its bundled standard library and notices, and needs no runtime Python. Release
  measurements cover startup, editing latency, stream throughput, and retained memory;
  correctness does not depend on noisy timing thresholds.

## Acceptance discipline

Every stage includes its tests, diagnostics, failure paths, and documentation updates.
Run the checks applicable to the changed contracts throughout the work; retain earlier
regressions at subsequent gates. Keep one current validation checkpoint with commands,
build identities, results, and remaining scope; avoid accumulating a second change log
in the specifications. Maintain a coverage map from contract IDs to real tests,
including partial/pending cases until their stage completes. The stage-4 matrix must
close every remaining coverage gap.
