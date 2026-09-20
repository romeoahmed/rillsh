# Testing

Tests establish observable contracts, not a particular heap layout, allocation count or
internal schedule. Read the owning specification and [status](status.md) when choosing
coverage. Cargo runs unit tests, owning-crate integration tests and rustdoc examples;
[development](development.md#quality-gate) gives the complete native gate.

## Evidence by boundary

| Owner              | Evidence                                                                                                                                                                     |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `rill-syntax`      | Lexical modes, whitespace/newline boundaries, byte spans, valid/invalid/incomplete input, structural validation and domain-aware properties                                  |
| `rill-runtime`     | Typed values/errors, currying, recursion, tail calls, nominal identity, graph equality, publication, import identity, GC across suspension, stream demand and release traces |
| `rill-system`      | Launch snapshots, child groups, redirects, native bytes, owned descriptors, filesystem identity, bounded transport and cleanup                                               |
| `rill-editor`      | Parser-backed validation, completion origins and quoting, native history exclusions and color selection                                                                      |
| `rillsh`           | Actual CLI output, useful diagnostics, process behavior, controlling-PTY interaction, cancellation, stop/resume and installation                                             |
| `tree-sitter-rill` | Reviewed corpus, execution-parser acceptance of valid examples, real query captures and incremental/fresh-tree equivalence                                                   |

A shared implementation is not an independent oracle. Properties specify their domain:
JSON round trips cover representable values; equality laws cover comparable values;
merge checks per-input order and accounting without requiring a particular interleaving.
Effectful callbacks are checked with observable traces, not assumed algebraic laws.

## Resources and scheduling

Use owned tempfile fixtures and subprocess-local environment/cwd. Explicit shutdown
joins workers and reaps children; Drop is a fallback. Process-global signal and terminal
cases run in separate processes. PTY fixtures create their own controlling terminal; a
missing capability is a failure of the test environment, never a silent skip.

Readiness messages and observed process state establish progress. Short polling backoffs
are acceptable within a deadline; elapsed sleep alone is not evidence. Tokio's paused
clock is suitable for timer logic, not real process progress. Pollable operations retain
partial transport, launch and decoder state across cancellation of a wait future.

Producer coverage includes no demand, failed acquisition, exhaustion, cutoff, failed
steps, failed release, nested ownership, interruption and suspension. A resource-backed
producer proves acquisition and release externally; an arithmetic generator does not.
Multi-input tests cover empty/unequal inputs, duplicate identities, blocked inputs,
backpressure, failures and cancellation. Collection while suspended checks traced state.
Controlled readiness tests cover source and operation waits, nested failure routing,
callback checkpoints and release completion after peer cutoff. Native tests cover
concurrent run/capture, shared job waiters, one foreground terminal lease, blocked
output cancellation and exact output resumption. Tiny-quanta tests collect between
native conversion steps; stable-sort properties check duplicate-key order against Rust's
standard sorter across multiple merge runs. JSON properties check source key order and
integer values independently of encoding; explicit cases reject escaped duplicate keys.
Constructor tests check reordered inputs and missing, extra or wrong keys. Record-rest
properties check unmatched values and insertion order independently of pattern order.

Inject failures at meaningful boundaries: a closed peer, failing child setup or a user
callback that raises. Avoid allocator-failure simulation, wholesale POSIX mocks,
descriptor-number assertions and private continuation counts.

## Interaction and diagnostics

PTY assertions inspect rendered content and cursor state with vt100. Cover empty,
partial and multiline Ctrl+C, repeated cancellation, the next submitted entry, Ctrl+D,
foreground handoff and stop/resume. Completion acceptance must edit without executing;
filename text must round-trip even with quotes, spaces, control bytes or `$()`.
Invalid-byte filenames are tested where the host filesystem permits them; native argv
bytes remain a cross-platform language contract.

Insta snapshots review source labels, original failure locations, Unicode columns and
secondary cleanup notes. Exact diagnostics are snapshot subjects deliberately; ordinary
semantic tests should assert structured errors rather than duplicate all wording. Review
updates before accepting them. CI never accepts snapshots automatically.

Keep executable fixtures in tests or corpus files. Do not extract test programs from
Markdown or assert documentation layout or example counts. Check documentation examples
manually when editing them; Rust API examples use native rustdoc tests.

The native editor owns its own paste and undo behavior. Test Rill's submitted-source,
completion and history policies without claiming incremental editor memory limits that
its library API cannot enforce.

## Properties, fuzzing and benchmarks

Proptest generators construct small valid domains with strategies and `prop_map`; avoid
discarding most samples with filters or `prop_assume!`. Keep empty, boundary and invalid
cases as explicit regressions alongside the properties. Semantic checks should not
require a program to finish in an exact number of VM instructions. Use ordinary
independent Rust values as the oracle, not a second invocation of the operation under
test. Integration tests persist failure seeds beside their source; unit tests use
Proptest's source-parallel default. Commit generated regression seeds and replay them
before random cases. A seed is tied to its strategy, so also retain a direct test for
each fixed bug. The [fuzz workspace](../fuzz/README.md) instruments syntax/lowering,
JSON, bounded pure evaluation, producers, UTF-8 lines and graph equality through
cargo-afl. Host effects are unavailable in evaluation fuzz targets. AFL++ checks committed
seeds during campaign startup; retain minimized failures as ordinary regression tests.
Finite fuzz domains run to completion; AFL++ owns hang detection,
not an arbitrary count of VM quanta inside the target.

Criterion workloads belong to their crates. Distinguish prepared VM work from complete
source-to-result execution and process pipelines. State whether timing includes setup,
GC, resource startup and destruction. Prepare fixtures outside timing unless their cost
is the subject. Run measurements without concurrent verification and record compiler,
profile, input and sample settings. Shared CI runners provide smoke evidence, not stable
microbenchmark rankings. Correctness tests do not contain performance thresholds.

Optimize measured workloads while preserving effect order, demand and cleanup. Keep
local measurements in ignored build directories; do not turn a historical result into
a permanent implementation constraint.
