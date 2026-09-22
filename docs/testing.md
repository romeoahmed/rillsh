# Testing

Test observable contracts and resource lifetimes. Use unit tests for local algorithms,
integration tests for owning-crate APIs, and the real CLI/PTY for session behavior.
Rustdoc examples cover public Rust APIs. The complete commands are in the
[quality gate](development.md#quality-gate).

## Coverage boundaries

| Owner              | Evidence                                                                                             |
| ------------------ | ---------------------------------------------------------------------------------------------------- |
| `rill-syntax`      | Lexical contexts, newlines, byte spans, complete/incomplete/invalid input and structural validation  |
| `rill-runtime`     | Values, errors, currying, recursion, nominal identity, publication, imports, GC and stream lifetimes |
| `rill-system`      | Launch snapshots, process groups, redirects, native bytes, descriptor ownership and transport        |
| `rill-editor`      | Validation, completion origins, history policy, highlighting and color selection                     |
| `rillsh`           | CLI effects, diagnostics, PTY editing, cancellation, stop/resume and installed execution             |
| `tree-sitter-rill` | Corpus, query captures, execution-parser agreement and incremental/fresh-tree equivalence            |

Keep executable fixtures in tests and corpus files. Do not read Markdown as test input
or assert its layout, example counts or wording. Check changed documentation examples
manually. Avoid exact allocation counts, descriptor numbers and private continuation
counts; internal tests may inspect relative reclamation when resource lifetime is the
behavior being tested.

## Effects, resources and GC

Use owned tempfile fixtures and subprocess-local environment/cwd. Isolate process-global
signal and terminal cases in subprocesses. PTY fixtures create a controlling terminal;
missing capability is an environment failure, not a silent skip. Explicitly shut down,
join workers and reap children; Drop remains a fallback.

Establish progress through readiness messages or observed process state. Polling within a
deadline is acceptable; elapsed sleep alone is not evidence. Inject failures at real
boundaries: a closed peer, failed child setup or a raising callback. Avoid wholesale
POSIX mocks and allocator-failure simulation.

Producer tests cover no demand, failed acquisition, exhaustion, cutoff, callback failure,
nested ownership, interruption and suspension. Resource-backed fixtures prove release
externally. Multi-input cases cover unequal inputs, duplicate identities, backpressure,
failure routing, foreground ownership and cleanup after peer cutoff. Verify exact output
across cancelled waits and stop/resume.

GC cases collect suspended work, retain published captures and reclaim discarded cycles.
Randomized quanta compare curried evaluation with independent arithmetic; allocating
native operations must permit debt-driven yields. Assert final values and effects, not
an exact instruction schedule or heap layout.

## Properties and diagnostics

Build small valid Proptest domains with strategies and `prop_map`, rather than rejecting
most samples. Keep explicit empty, boundary and invalid regressions. Use independent
Rust values as oracles: stable-sort properties compare key order, record-rest properties
compare unmatched fields, and JSON properties check numbers and key order independently
of encoding. Encoding errors retain precise paths after sibling containers and GC yields.
Aggregation compares first-key and item order with a linear model.
Equality laws require comparable values; merge requires per-input order,
not a fixed interleaving. Effectful callbacks need traces, not assumed algebraic laws.

Persist and commit minimized regression seeds beside their tests. Seeds depend on the
strategy, so retain a direct regression for each fixed bug. Insta snapshots deliberately
cover diagnostic wording, original source locations, Unicode columns and cleanup notes;
other tests should prefer structured errors. Review snapshot updates; never auto-accept
in CI.

PTY assertions use rendered content and cursor state. Cover cancellation, EOF, multiline
editing, history search, completion acceptance without execution and terminal handoff.
Completed filename insertion must denote exactly one path/argument; exercise directory
prefix continuation and insertion within an existing command separately. Completion
must respect lexical scope and escape documentation without altering explicit help data.
External-editor tests include atomic saves and acceptance
without execution. Source-tool tests prove checking and formatting cannot run effects or
resolve imports. Dynamic flat-map tests cover cleanup before the next inner source,
cutoff through wrappers, failure and cancellation. Test Rill policies without
claiming paste or undo limits that Reedline cannot enforce.

## Fuzzing and performance

The isolated [fuzz workspace](../fuzz/README.md) owns AFL++ targets, domains and replay
instructions. Evaluation targets expose no host effects. AFL++ checks seeds and detects
hangs; harnesses do not impose a second VM-instruction deadline. Keep minimized findings
as ordinary regressions. Short campaigns are supplementary evidence.

[Criterion workloads](development.md#measurements) distinguish prepared VM execution
from source-to-result and process pipelines. State whether setup, GC and destruction
are timed. Optimize measured workloads while preserving effect order, demand and cleanup;
never turn one historical measurement into a correctness requirement.
