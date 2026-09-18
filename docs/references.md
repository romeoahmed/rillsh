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
- [RFC 8259](https://www.rfc-editor.org/rfc/rfc8259): JSON syntax. Rill's codec
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
  [wraps](https://mesonbuild.com/Wrap-dependency-system-manual.html),
  [1.12 release notes](https://mesonbuild.com/Release-notes-for-1-12-0.html), and
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
introduction: purpose, runnable examples, setup, and navigation.

## Language and interaction ideas

- [R7RS](https://standards.scheme.org/r7rs-html5/index.html): first-class functions,
  lexical scope, and proper tail recursion.
- [Chez Scheme editor](https://cisco.github.io/ChezScheme/csug10.0/use.html):
  whole-expression input and history.
- [Haskell expressions](https://www.haskell.org/onlinereport/haskell2010/haskellch3.html):
  unary application, currying, and patterns; Rill evaluates strictly.
- Nushell's [PipelineData](https://github.com/nushell/nushell/blob/main/crates/nu-protocol/src/pipeline/pipeline_data.rs)
  and [ByteStream](https://github.com/nushell/nushell/blob/main/crates/nu-protocol/src/pipeline/byte_stream.rs):
  values, consumable streams, and process bytes.
- fish's [reader](https://github.com/fish-shell/fish-shell/blob/master/src/reader/reader.rs)
  and [highlighting](https://github.com/fish-shell/fish-shell/blob/master/src/highlight/highlight.rs):
  responsive input and rejection of stale asynchronous results.

These inform specific choices; they are not compatibility promises.

## Runtime memory design

The [runtime design](architecture.md#values-and-memory) owns the selected algorithms.
These sources provide concrete implementations and alternative tradeoffs:

- [V8 collector overview](https://v8.dev/blog/trash-talk) and
  [Lua's collector](https://www.lua.org/source/5.5/lgc.c.html): generations, barriers,
  relocation, and incremental tracing.
- [Lua parser](https://www.lua.org/source/5.5/lparser.c.html) and
  [closure ownership](https://www.lua.org/source/5.5/lfunc.c.html): separate function
  code, lexical analysis, and closure instances.
- [LLVM GC integration](https://llvm.org/docs/GarbageCollection.html) and
  [statepoints](https://llvm.org/docs/Statepoints.html): roots, safepoints, barriers,
  and relocation of derived pointers. Rill keeps its explicit nonmoving root contract.
- [Princeton sorting applications](https://algs4.cs.princeton.edu/25applications/):
  duplicate detection by sorting and scanning adjacent keys. Rill uses libc sorting over
  borrowed pointers to preserve source-order evaluation.
- [LLVM Programmer's Manual](https://llvm.org/docs/ProgrammersManual.html): allocation
  for objects sharing a lifetime.
- [Cornell amortized analysis](https://www.cs.cornell.edu/courses/cs3110/2014fa/lectures/25/lec25.html):
  geometric buffer growth; graph charging and GC have separate costs.
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
- Leijen, Zorn, and de Moura, [Mimalloc: Free List Sharding in Action](https://www.microsoft.com/en-us/research/publication/mimalloc-free-list-sharding-in-action/):
  page-local free lists and allocator locality. Rill retains libc allocation; fewer
  allocations do not by themselves establish fewer CPU cache misses.
- Cornell, [Graph Traversals](https://courses.cis.cornell.edu/courses/cs2110/2026sp/lectures/lec22/):
  discovery marking bounds shared/cyclic reachability to vertex-and-edge work. Rill's
  temporary intrusive queue uses this principle without a separate visited hash table.
- Blackburn and McKinley, [Immix](https://www.mmtk.io/assets/pubs/immix-pldi-2008.pdf):
  block/line allocation, recycling, fragmentation, and opportunistic evacuation explain
  why a region allocator requires more than a bump pointer.
- Okasaki, [Purely Functional Data Structures](https://www.cs.cmu.edu/~rwh/students/okasaki.pdf):
  persistent versions require appropriate amortized reasoning; array, tree, and lazy
  representations have different costs. Rill retains strict immutable arrays and slices.

## Bytecode and program analysis

- Cornell, [Redundancy elimination](https://www.cs.cornell.edu/courses/cs4120/2026sp/notes.html?id=redund_elim):
  available expressions and value numbering. Rill precomputes structural metadata;
  arbitrary calls, errors, and resource boundaries cannot be treated as pure expressions.
- Shi et al., [Virtual Machine Showdown: Stack Versus Registers](https://www.cs.tufts.edu/comp/150FP/archive/david-gregg/vm-showdown.pdf):
  dispatch count, operand traffic, and instruction size tradeoffs. Its measured JVM
  workloads do not predict Rill's speedup.
- [Lua 5.5 execution loop](https://www.lua.org/source/5.5/lvm.c.html) and
  [instruction formats](https://www.lua.org/source/5.5/lopcodes.h.html): register
  operands, calls, and interpreter state. Rill borrows design questions, not Lua's
  language semantics or instruction encoding.
- Cornell, [Functional programming and closure conversion](https://www.cs.cornell.edu/courses/cs4120/2026sp/notes.html?id=functional):
  lexical environments, escaping bindings, and closure representation. Rill's cached
  free-name summaries and prepared capture slots preserve lexical bindings and instance
  lifetimes without adding an optimizer framework.
- Tufts, [VM garbage collection](https://www.cs.tufts.edu/cs/106/modules/11gc.html):
  register roots, liveness, relocation, and interior instruction pointers. A bytecode
  conversion must preserve these ownership obligations as well as evaluation results.
