# References

Rill's specifications own its syntax and policy. These primary sources explain the
selected mechanisms. Cargo manifests and lockfiles own dependency versions;
[status](status.md) records evidence and remaining work.

## Standards and libraries

- [Rust Reference](https://doc.rust-lang.org/stable/reference/),
  [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html),
  [profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) and
  [Clippy](https://doc.rust-lang.org/stable/clippy/lints.html): language, build and lint policy.
- [POSIX.1-2024](https://pubs.opengroup.org/onlinepubs/9799919799/),
  [rustix](https://docs.rs/rustix/latest/rustix/) and
  [Tokio](https://docs.rs/tokio/latest/tokio/): Unix ownership and asynchronous coordination.
  Using these APIs does not imply POSIX shell grammar compatibility.
- [XDG Base Directory](https://specifications.freedesktop.org/basedir-spec/latest/) and
  [xdg](https://docs.rs/xdg/latest/xdg/): startup and persistent state locations.
- [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259),
  [Serde](https://serde.rs/) and [serde_json](https://docs.rs/serde_json/latest/serde_json/):
  JSON visitors and serializers; Rill additionally rejects duplicate keys and invalid numbers.
- [Logos](https://logos.maciej.codes/) and
  [Chumsky](https://docs.rs/chumsky/latest/chumsky/): contextual tokenization and parsing.
- [gc-arena](https://docs.rs/gc-arena/latest/gc_arena/): traced cycles, mutation barriers
  and scoped arena access. GC does not own OS resource lifetimes.
- [num-traits](https://docs.rs/num-traits/latest/num_traits/cast/trait.ToPrimitive.html):
  checked Float-to-Int conversion without handwritten representability bounds.
- [Bytes](https://docs.rs/bytes/latest/bytes/struct.Bytes.html),
  [IndexMap](https://docs.rs/indexmap/latest/indexmap/) and
  [SlotMap](https://docs.rs/slotmap/latest/slotmap/): shared byte slices, ordered fields
  and generational resource identities.
- [Reedline](https://docs.rs/reedline/latest/reedline/),
  [unicode-segmentation](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/),
  [unicode-width](https://docs.rs/unicode-width/latest/unicode_width/) and
  [Ariadne](https://docs.rs/ariadne/latest/ariadne/): editing, display and diagnostics.
- [Tree-sitter](https://tree-sitter.github.io/tree-sitter/): grammar packages, queries,
  generation and incremental edit protocols.

## Verification and contribution

- [Rust test organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html),
  [Proptest](https://proptest-rs.github.io/proptest/intro.html),
  [Insta](https://insta.rs/docs/quickstart/) and
  [Criterion](https://docs.rs/criterion/latest/criterion/): native tests and measurements.
- [cargo-afl](https://github.com/rust-fuzz/afl.rs): isolated AFL++ instrumentation and campaigns.
- [Checkout](https://github.com/actions/checkout),
  [Rust toolchain](https://github.com/dtolnay/rust-toolchain) and
  [Rust Cache](https://github.com/Swatinem/rust-cache): native CI setup.
- [Rust comments](https://doc.rust-lang.org/reference/comments.html),
  [rustdoc](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html), [AGENTS.md](https://agents.md/) and
  [Awesome README](https://github.com/matiassingers/awesome-readme): API contracts,
  actionable agent instructions and a user-facing project introduction.

## Design influences

[R7RS](https://standards.scheme.org/r7rs-html5/index.html) informs lexical scope and
proper tail recursion; [Haskell
expressions](https://www.haskell.org/onlinereport/haskell2010/haskellch3.html) inform
unary application and patterns, while Rill remains strict.
[Nushell](https://github.com/nushell/nushell) and
[fish](https://github.com/fish-shell/fish-shell) inform explicit data pipelines and
responsive interaction. These are design sources, not compatibility promises or
alternative runtime dependencies.
