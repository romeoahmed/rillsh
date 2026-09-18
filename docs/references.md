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
  the planned config/state layout on both platforms.
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

[yyjson](https://github.com/ibireme/yyjson) is the sole third-party runtime library.
Its [API reference](https://ibireme.github.io/yyjson/doc/doxygen/html/api.html) defines
JSON document ownership; Rill owns conversion and validation. The
[wrap](../subprojects/yyjson.wrap) pins the reviewed release. There is no third-party
line editor.

The [Doxygen manual](https://www.doxygen.nl/manual/index.html),
[comment syntax](https://www.doxygen.nl/manual/docblocks.html), and
[configuration reference](https://www.doxygen.nl/manual/config.html) guide declaration-local
contracts and the generated C reference. [AGENTS.md](https://agents.md/) informs the
separate coding-agent guide. [Awesome README](https://github.com/matiassingers/awesome-readme)
informs the user-facing introduction: purpose, working examples, setup, and navigation.

## Language and interaction ideas

| Source                                                                                                                                                                                                                              | Idea retained                                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [R7RS](https://standards.scheme.org/r7rs-html5/index.html)                                                                                                                                                                          | First-class functions, lexical scope, proper tail recursion        |
| [Chez Scheme editor](https://cisco.github.io/ChezScheme/csug10.0/use.html)                                                                                                                                                          | Whole-expression input and history                                 |
| [Haskell expressions](https://www.haskell.org/onlinereport/haskell2010/haskellch3.html)                                                                                                                                             | Unary application, currying, patterns; Rill evaluates strictly     |
| [Nushell PipelineData](https://github.com/nushell/nushell/blob/main/crates/nu-protocol/src/pipeline/pipeline_data.rs) and [ByteStream](https://github.com/nushell/nushell/blob/main/crates/nu-protocol/src/pipeline/byte_stream.rs) | Reusable values versus consumable streams; process byte transport  |
| [fish reader](https://github.com/fish-shell/fish-shell/blob/master/src/reader/reader.rs) and [highlighting](https://github.com/fish-shell/fish-shell/blob/master/src/highlight/highlight.rs)                                        | Responsive input, stale-result rejection, nonblocking highlighting |

These are design references, not compatibility promises.
