# Fuzzing

This independent Cargo workspace keeps AFL++ instrumentation and its lockfile outside
normal shell builds. Targets reset their engine for every input. Syntax and JSON reject
inputs above 8 KiB inside the harness, including direct seed replay; the remaining
targets use the smaller domains described below. Evaluation runs to completion;
AFL++ detects hangs through its execution timeout, without a second instruction-count
limit in the target:

- `syntax`: UTF-8 decoding, contextual lexing, completion-span boundaries, quoted-path round trips, parsing, validation and bytecode lowering.
  It never evaluates source or loads imports.
- `json`: arbitrary byte decoding and round trips over JSON-representable values, with
  collection between VM quanta. No generated program can request a host effect.
- `evaluation`: bounded curried recursion and list-pattern evaluation, checked against
  independent integer arithmetic while collecting suspended state and interleaving
  unrelated entries with parked continuations. This restricted
  domain avoids treating an instruction budget as a native-operation or memory bound.
- `producer`: finite demand and state transitions, cutoff versus exhaustion, and lazy
  acquisition, with release invariants checked against an independent integer model
  under collection. It creates no OS resources and requests no host effects.
- `lines`: chunked UTF-8 decoding against an independent byte-record model, including
  CRLF, final fragments, invalid encoding and line-byte limits under collection. The
  first two bytes select chunk size (1–17) and line limit (0–64); the remaining bytes
  are payload. Inputs contain 2–4,096 bytes. No host effects are available.
- `equality`: shared acyclic Lists/Records compared with independently expanded Rust
  trees under collection, including reordered fields, reflexivity, symmetry and `!=`.
  Inputs contain 2–74 bytes: two root selectors followed by up to twelve six-byte node
  records, with three bytes per graph selecting a kind and earlier children or a leaf.
  The small tree oracle has no graph memoization and requests no host effects.

Follow [cargo-afl](https://github.com/rust-fuzz/afl.rs) for platform prerequisites:

```sh
cargo install cargo-afl --locked
cargo afl build --manifest-path fuzz/Cargo.toml --locked
cargo afl fuzz -i fuzz/corpus/syntax -o fuzz/target/findings/syntax -V 60 -G 8192 -- fuzz/target/debug/syntax
```

Replace `syntax` with `json`, `evaluation`, `producer`, `lines` or `equality` for the
other targets. Read AFL++'s host checks before running; system configuration changes are
not part of the normal Cargo gate. Native Linux CI owns the instrumented smoke
campaigns. Replay an exact seed by passing it on standard input to its built target.

Keep generated findings under `fuzz/target/`. Minimize failures and commit a normal Rust
regression with a reviewed fixture. A short campaign is supplementary evidence, not
proof of semantic completeness or memory safety.
