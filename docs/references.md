# References

Rill Shell's specifications own its syntax and policy. These upstream sources explain
the standards, mechanisms, and design ideas behind them. Dependency and Unicode
manifests own exact pins; [status](status.md) records the validation checkpoint.

## Standards

- [ISO/IEC 9899:2024](https://www.iso.org/standard/82075.html) and the public
  [C23 draft](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n3096.pdf): language and
  library baseline. A compiler mode does not establish complete libc support.
- [POSIX.1-2024](https://pubs.opengroup.org/onlinepubs/9799919799/): processes, signals,
  descriptors, environment, filesystems, and terminal control. Rill Shell uses these
  interfaces without adopting POSIX shell syntax or claiming full certification.
- [XDG Base Directory 0.8](https://specifications.freedesktop.org/basedir-spec/0.8/):
  configuration and the planned persistent-state layout on both platforms.
- [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259): JSON syntax. Rill's planned codec
  additionally rejects duplicate keys and out-of-range numbers.
- [Unicode 18.0.0](https://www.unicode.org/Public/18.0.0/ucd/ReadMe.txt),
  [UAX #29 revision 49](https://www.unicode.org/reports/tr29/tr29-49.html),
  [UAX #11](https://www.unicode.org/reports/tr11/), and
  [UTS #51 revision 31](https://www.unicode.org/reports/tr51/tr51-31.html): character
  properties, segmentation, width inputs, and emoji presentation. The tracked
  [manifest](../data/unicode/manifest.json) lists generator inputs and hashes;
  full editor width support needs the corresponding sequence data as well.
- [xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html):
  the selected VT profile, color, and bracketed paste.
  [terminfo](https://invisible-island.net/ncurses/man/terminfo.5.html) describes the
  database approach excluded from this profile.

The [Linux userspace guide](https://docs.kernel.org/userspace-api/index.html) is useful
for Linux-specific behavior. Kernel-internal APIs are not this shell's interface.

## Build and analysis

- [Meson reference](https://mesonbuild.com/Reference-manual.html),
  [built-in options](https://mesonbuild.com/Builtin-options.html),
  [wraps](https://mesonbuild.com/Wrap-dependency-system-manual.html), and
  [unit tests](https://mesonbuild.com/Unit-tests.html): native build and test facilities.
- [GCC extensions](https://gcc.gnu.org/onlinedocs/gcc/C-Extensions.html),
  [attributes](https://gcc.gnu.org/onlinedocs/gcc/Common-Attributes.html), and
  [warnings](https://gcc.gnu.org/onlinedocs/gcc/Warning-Options.html): GNU C23 choices.
- [GCC variable attributes](https://gcc.gnu.org/onlinedocs/gcc/Common-Variable-Attributes.html):
  typed `cleanup` callbacks and their scope-exit guarantees.
- [C23 binary resource inclusion](https://gcc.gnu.org/onlinedocs/cpp/Binary-Resource-Inclusion.html):
  native embedding of bundled standard-library source.
- [Clang C support](https://clang.llvm.org/c_status.html),
  [extensions](https://clang.llvm.org/docs/LanguageExtensions.html),
  [attributes](https://clang.llvm.org/docs/AttributeReference.html), and
  [diagnostics](https://clang.llvm.org/docs/DiagnosticsReference.html): the other
  supported compiler and its diagnostic groups.
- [clang-tidy](https://clang.llvm.org/extra/clang-tidy/),
  [clang-format](https://clang.llvm.org/docs/ClangFormatStyleOptions.html),
  [ASan](https://clang.llvm.org/docs/AddressSanitizer.html),
  [UBSan](https://clang.llvm.org/docs/UndefinedBehaviorSanitizer.html), and
  [libFuzzer](https://llvm.org/docs/LibFuzzer.html): analysis and validation tools.
- [Python 3.14 functional HOWTO](https://docs.python.org/3.14/howto/functional.html),
  [pathlib](https://docs.python.org/3.14/library/pathlib.html), and
  [unittest](https://docs.python.org/3.14/library/unittest.html): transformations,
  paths, and test ownership. [Ruff](https://docs.astral.sh/ruff/configuration/) and
  [ty](https://docs.astral.sh/ty/) provide development checks.

Tool-selection rationale and nondefault settings belong in
[development](development.md), not in a second configuration inventory here.

## Dependencies and documentation

[yyjson](https://github.com/ibireme/yyjson) is the sole third-party runtime library. Its
[API reference](https://ibireme.github.io/yyjson/doc/doxygen/html/api.html) defines JSON
document ownership; Rill owns conversion and validation. The
[wrap](../subprojects/yyjson.wrap) pins the reviewed release. There is no third-party
line editor.

The [Doxygen manual](https://www.doxygen.nl/manual/index.html), [comment
syntax](https://www.doxygen.nl/manual/docblocks.html), and [configuration
reference](https://www.doxygen.nl/manual/config.html) guide declaration-local contracts
and the generated C reference. [AGENTS.md](https://agents.md/) informs the separate,
actionable build and change instructions in the coding-agent guide. [Awesome
README](https://github.com/matiassingers/awesome-readme) informs the user-facing
introduction: purpose, working examples, setup, and navigation. These are organizational
references, not requirements to add badges, banners, or duplicate documentation.

## Language and interaction ideas

| Source                                                                                                                                                                                                                              | Idea retained                                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [R7RS](https://standards.scheme.org/r7rs-html5/index.html)                                                                                                                                                                          | First-class functions, lexical scope, proper tail recursion        |
| [Chez Scheme editor](https://cisco.github.io/ChezScheme/csug10.0/use.html)                                                                                                                                                          | Whole-expression input and history                                 |
| [Haskell expressions](https://www.haskell.org/onlinereport/haskell2010/haskellch3.html)                                                                                                                                             | Unary application, currying, patterns; Rill evaluates strictly     |
| [Nushell PipelineData](https://github.com/nushell/nushell/blob/main/crates/nu-protocol/src/pipeline/pipeline_data.rs) and [ByteStream](https://github.com/nushell/nushell/blob/main/crates/nu-protocol/src/pipeline/byte_stream.rs) | Reusable values versus consumable streams; process byte transport  |
| [fish reader](https://github.com/fish-shell/fish-shell/blob/master/src/reader/reader.rs) and [highlighting](https://github.com/fish-shell/fish-shell/blob/master/src/highlight/highlight.rs)                                        | Responsive input, stale-result rejection, nonblocking highlighting |

These are design references, not compatibility promises.

## Runtime memory design

[V8's collector overview](https://v8.dev/blog/trash-talk) explains generational roots,
write barriers, evacuation, and compaction tradeoffs. It informs the comparison in
[architecture](architecture.md); Rill retains a single-threaded, nonmoving collector.
[Meson benchmarks](https://mesonbuild.com/Unit-tests.html#benchmarks) provide separate,
serial performance runs. Neither source prescribes a universal allocator or collector
for Rill's workloads.

[Lua 5.4 parser and upvalue analysis](https://www.lua.org/source/5.4/lparser.c.html) and
[closure/prototype ownership](https://www.lua.org/source/5.4/lfunc.c.html) illustrate
separating function code from closure instances. Rill uses code-owned free-name
metadata, not Lua's bytecode or mutable upvalue model. The [LLVM Programmer's
Manual](https://llvm.org/docs/ProgrammersManual.html) discusses bump allocation for
groups of objects sharing a lifetime; Rill uses small typed syntax blocks without
importing a general allocator framework.

- [Chez Scheme storage management](https://cisco.github.io/ChezScheme/csug/smgmt.html),
  [collector source](https://github.com/cisco/ChezScheme/blob/main/c/gc.c), and
  [allocation source](https://github.com/cisco/ChezScheme/blob/main/c/alloc.c): generations,
  relocation, marking, and dirty-object handling. These mechanisms depend on the runtime's
  allocation and pointer contracts.
- Keep, Hearn, and Dybvig, [Optimizing Closures in O(0) Time](https://andykeep.com/pubs/scheme-12a.pdf):
  flat captures, recursive dependencies, and the space-safety constraints on sharing.
  Rill borrows the representation principles, not the paper's complete compiler pass.
- Shao and Appel, [Efficient and Safe-for-Space Closure Conversion](https://doi.org/10.1145/345099.345125):
  closure layout must preserve lifetimes as well as reduce allocation and indirection.
- Adams and Dybvig, [Efficient Nondestructive Equality Checking for Trees and Graphs](https://michaeldadams.org/papers/efficient_equality/):
  bounded traversal and equivalence classes avoid repeated DAG expansion. The
  [Chez implementation](https://github.com/cisco/ChezScheme/blob/main/s/5_1.ss) interleaves
  tree traversal and union-find. Rill uses a bounded pre-check and union by rank/path
  halving after full data validation, preserving its different error and cycle contracts.
- Princeton, [Union-Find](https://algs4.cs.princeton.edu/15uf/): rank/weight balancing,
  path compression, and amortized analysis. Hashing and byte comparison costs must be
  accounted for separately from disjoint-set operations.
- Wilson et al., [Dynamic Storage Allocation: A Survey and Critical Review](https://csapp.cs.cmu.edu/3e/docs/dsa.pdf):
  allocation policy, fragmentation, and realistic workload evaluation. Requested bytes,
  resident memory, allocation traffic, and locality are distinct measurements.
- Blackburn and McKinley, [Immix](https://www.mmtk.io/assets/pubs/immix-pldi-2008.pdf):
  block/line allocation, recycling, fragmentation, and opportunistic evacuation explain
  why a region allocator requires more than a bump pointer.
- Okasaki, [Purely Functional Data Structures](https://www.cs.cmu.edu/~rwh/students/okasaki.pdf):
  persistent versions require appropriate amortized reasoning; array, tree, and lazy
  representations have different costs. Rill retains strict immutable arrays and slices.
