# Repository and implementation plan

This document maps the [architecture](architecture.md) to source ownership and four
implementation stages. [Language](language.md), [execution](execution.md),
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
  syntax/                     Lexer, parser, syntax metadata, source-level AST
  runtime/                    Values, GC, resolution/lowering, continuations, stream ownership
  exec/                       Launch preparation, process groups, jobs, reports, byte pumps
  platform/                   Cohesive POSIX services and actual Linux/macOS differences
  editor/                     Planned: decoder, edits/undo, layout, rendering
  library/                    Native primitives, standard-library registration, value adapters
stdlib/
  prelude.rill
  core.rill
  seq.rill
  text.rill
  fs.rill                     Planned filesystem API
  process.rill
  json.rill                   Planned JSON API
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

`tools/` contains the Unicode generator/verifier and the small check driver shared by
developers and CI; both use Python's standard library. C23 `#embed` embeds the standard
library directly. `tests/unit/` holds C component tests, `system/` holds Python
process/PTY tests, `helpers/` holds controlled child programs, and `benchmarks/` holds
release workloads. Add `fuzz/` with the fuzz targets; add `spec/` only if standalone
Rill fixtures improve conformance testing. Keep fixtures with the cases that use them.
Meson suite names remain those in testing; directories and suites need not correspond
one-to-one. Generated build files and benchmark results stay under ignored build trees.

Keep headers beside their implementations. There is no installed C SDK or public
`include/` tree. `src/main.c` is the only executable entry point.

## Module boundaries

The session assembles concrete components. Each component owns a domain and its
invariants; it does not receive the entire Session merely to obtain unrelated services.
Source and diagnostic types are small shared contracts, not the seed of a `utils` layer.
The following table lists allowed project dependencies; standard C facilities are
available everywhere, and source/diagnostic types are omitted from individual rows.

| Owner | Responsibility | Direct component dependencies |
| --- | --- | --- |
| `text` | Length-aware text, Unicode properties/metrics, literal/style spans | None |
| `syntax` | Syntax, completeness, precedence, AST, highlight/indentation metadata | `text` |
| `runtime` | Language values, code ownership, lexical resolution, GC, evaluation, resource tokens | `syntax`, `text` |
| `platform` | Native descriptor, terminal, signal, filesystem, environment, and clock services | `text` where byte/text views are needed |
| `exec` | Prepared launches, child identities, job state, reports, nonblocking byte transport | `platform`, `text` |
| `editor` | Pure input/edit/layout state and rendering operations | `syntax`, `text` |
| `library` | Callable primitives, codecs, filesystem/process adapters, library metadata | `runtime`, `exec`, `platform`, `text`; private yyjson |
| Session | Scheduling, terminal handoff, startup, module I/O, history/completion coordination, presentation | Component interfaces above |

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
transform, and sink state. Implementations can request a language call, wait for an
interest, or complete; the session schedules progress. States retaining Values expose
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
implementation size or tests justify it. For example, `runtime/value.c`, `heap.c`,
`eval.c`, and `stream.c` are useful boundaries; a separate file for each value variant
or evaluator opcode is not. `exec/launch.c` and `job.c` separate launch setup from live
supervision; `editor/input.c`, `edit.c`, `layout.c`, and `render.c` separate testable
state machines. These examples are starting points, not an exhaustive file checklist.

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
and component dependencies. Reuse private static targets between the shell and tests.
Pure runtime/editor tests link without the supervisor or yyjson. Add targets and build
fragments for reuse or clear ownership, not one per directory. Meson dependency objects
carry include paths and link requirements.

Maintain one list of boundary headers for the optional `api-docs` target. The quality
gate requires it; ordinary shell builds do not. Cross-component includes must follow the
dependency table: include paths do not enforce encapsulation. The quality gate checks
project boundaries, while review guards against private-representation dependencies.

| Input | Output | Lifecycle |
| --- | --- | --- |
| Pinned Unicode data and manifest | `src/text/unicode_tables.inc` | Explicit regeneration; both inputs and output tracked |
| Authored `stdlib/*.rill` | Read-only bytes in the executable | Native C23 `#embed`; compiler-tracked dependencies |

Bundling gives `std:` modules a deterministic source versioned with the executable.
Diagnostics use logical names such as `std:seq`; file imports retain their own identity
cache and relative-path rules. There is no installed-file search or cwd fallback for
bundled modules.

`src/library/` owns representation access, OS effects, and codecs; `stdlib/` expresses
composition and defaults in Rill. Bootstrap the private native registry, load bundled
modules, and install the prelude through the same unary application protocol. Builtin
help/signatures belong beside native registration, grammar metadata in `syntax`, and
Unicode properties in `text`. [Development](development.md) owns dependency acquisition,
generation checks, and tool configuration.

## Implementation stages

The four stages are substantial integration milestones. Each delivers a runnable program
with acceptance evidence and extends the same parser, evaluator, supervisor, and event
loop. Commit size is independent of stage boundaries.

### 1. A reliable executable shell foundation

**Complete.** Implemented scope and acceptance evidence: [current status](status.md).

**Deliver.** Establish the repository/build layout, Fedora development image, native
macOS builds, strict toolchain, API documentation target, pinned inputs, Unicode
generation, and test helpers. Implement the production source/diagnostic path, initial
Values/GC and continuation protocol, command parsing/lowering, and the POSIX supervisor
with its byte pumps. Support literal commands, byte pipelines, redirection, launch
reports, cancellation, and external job control. Use the same unary native-call path
that later language features will extend. Provide CLI/script input and a canonical-input
prompt; text/style separation and RGB capability policy start here even though
cursor-addressed editing comes later.

**Boundary.** This is the command-capable subset of the final grammar. Closures,
user-defined ADTs, structured streams, and rich editing are not completion conditions.
Process tests can construct launch specifications directly before all public library
functions exist. No temporary shell interpreter or `system()` fallback is acceptable.

**Accept when:**

- The actual binary executes a multi-stage pipeline from source and reports syntax,
  launch, and process failures distinctly; controlled helpers verify bytes and redirection.
- Process contracts E03–E08, E14–E16, E18–E20 and platform P01–P03, P07, P13–P14 pass
  for the implemented command surface, using direct supervisor tests where necessary.
  External foreground/background stop/resume and terminal restoration have real PTY evidence.
- Fedora and native macOS builds pass compilation, formatting, analysis, and applicable
  ASan/UBSan/release tests. The Unicode corpus and minimal RGB/plain rendering checks pass.
  Documented boundary headers generate a warning-free HTML reference with C23 declarations
  and ownership contracts intact.

### 2. The complete functional language

**Complete.** Implemented scope and acceptance evidence: [current status](status.md).

**Deliver.** Complete syntax, lexical resolution/lowering, first-class unary functions,
currying, proper tail calls, records, ADTs, patterns, arithmetic, error conversion,
modules, and transactional REPL binding publication. Complete code/capture ownership, GC
roots, and collection of shadowed bindings. Bootstrap bundled modules and the prelude;
implement pure sequence/text operations and the public JobPlan/process API through the
existing adapters. Extend canonical multiline input to the complete grammar and add
startup config.

**Boundary.** The language and reusable plan model are complete. Structured streaming,
stream-dependent lifetime cases, and suspended mixed evaluations belong to stage 3; the
editor still uses the plain input path.

**Accept when:**

- First-class closures and constructors can be stored, returned, partially applied, and
  matched across module imports and REPL entries, with specified effect order.
- L01–L14 and A01–A12 pass for non-Stream cases, including one million tail calls,
  stress collection, and live-heap checks; E01–E02 verify plan reuse and launch snapshots.
  Stream-specific branches of L10–L11 and L15–L16 are explicitly pending stage 3.
- End-to-end scripts combine library functions and external plans without a second
  dispatch path. Failed entries preserve earlier effects but publish no partial bindings;
  original stage-1 process and terminal regressions remain green.

### 3. Structured streaming and complete execution semantics

**Deliver.** Implement stream ownership/escape checks, explicit close/finalization,
source/transform/sink resumption connected to the existing byte pumps, codecs, Stream
extensions to the existing collection/sorting budgets, and the filesystem/JSON library.
Complete `capture`, `stream`, `through`, and checked completion. Integrate cancellation
and suspended evaluator/job contexts with foreground resume, session snapshots, and
shutdown. Complete all library contracts and render results incrementally through
text/style spans on the existing prompt.

**Boundary.** This is the complete language and execution model usable through scripts
and plain interaction. Rich editing, persistent history, and completion UI remain stage
4. There is still only one active user evaluator; no background callback scheduler is
added.

**Accept when:**

- All L-, A-, and E-series contracts pass, including previously pending Stream branches.
  JSON/Unicode boundaries and limit options have property, fuzz, and failure-injection
  evidence; cancelled reports cannot become success merely because children exit zero.
- A filesystem value stream can be transformed, materialized, encoded, and passed through
  an external process; a long process stream can be cut off without zombies or hidden
  producer failures. Large simultaneous input/output/stderr traffic stays bounded.
- PTY scenarios interrupt and suspend both pure evaluation and mixed pipelines. `fg`
  resumes correctly, unsupported `bg` is rejected without losing state, and repeated
  failure/cancellation leaves the prompt, descriptors, and heap usable.

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

- The complete set of 74 acceptance contracts and all named Meson suites pass on the
  required matrix; parser/editor/codec fuzz targets run under sanitizers, and examples
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
