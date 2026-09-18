# Development

This guide owns build, implementation, and documentation conventions.
[Current status](status.md) identifies the available feature set;
[testing](testing.md) defines behavior and acceptance evidence.

## Build and test

Use Linux/glibc or macOS with GNU C23, Meson 1.12 or newer, Ninja, and Python 3.14 or
newer. GCC and Clang are supported; choose with `CC` at initial setup. The selected
compiler and C library must provide the facilities used by the source, including
`<stdckdint.h>`. Meson selects the language mode; no duplicate feature-probe catalogue
or compiler-version gate is needed.

Acquire dependencies explicitly, then configure without network access:

```sh
meson subprojects download
CC=clang meson setup build/asan --buildtype=debugoptimized \
  --wrap-mode=nodownload -Db_sanitize=address,undefined
meson compile -C build/asan
meson test -C build/asan --print-errorlogs

meson setup build/release --buildtype=release --wrap-mode=nodownload
meson compile -C build/release
meson test -C build/release --print-errorlogs
```

Keep separate build directories for compilers, profiles, and operating systems. Use a
consistent LLVM release for Clang, clang-format, clang-tidy, and sanitizer runtimes.
On macOS, supply `SDKROOT` from `xcrun --show-sdk-path` when analysis needs the active
SDK; do not store the resolved path in project files.

ASan/UBSan is the default correctness profile. Meson supplies frame-pointer flags;
undefined-behavior recovery is disabled in the project configuration. Preserve Meson's
sanitizer test environment and provide the matching symbolizer. Verify leak-checking
support per platform. Fuzz targets use a separate libFuzzer build; production binaries
do not link it. MSan requires an instrumented environment; TSan is not a default for
this single-threaded runtime. Measure release performance separately.

### Optional Fedora environment

The root `Containerfile` provisions `quay.io/fedora/fedora:45` with GCC, LLVM, Meson,
Ninja, Doxygen, Python, and development headers. Use a compatible OCI builder with the
project root as context; `.containerignore` excludes local outputs. Mount the checkout
at `/work` when running the image. No particular container runtime is required.

Keep credentials and source checkouts out of the image. Record image digests, tool
versions, architecture, and results with validation evidence. Linux tests do not
establish native macOS support; unavailable matrix entries remain unverified.

## Quality gate

With LLVM tools and Doxygen in PATH, run from the repository root:

```sh
python3 tools/check.py build/asan
```

The driver checks repository privacy, Markdown links/table structure, component include
boundaries, deterministic Unicode generation, clang-tidy configuration, formatting,
static analysis, API documentation, and the Meson tests. It uses Meson's native tool
targets and compile database. Review Markdown anchors and runnable examples as well;
the lightweight checks do not establish semantic correctness.

For Python changes:

```sh
uvx ruff check tools tests/system
uvx ruff format --check tools tests/system
uvx ty check tools tests/system
```

Format locally with `uvx ruff format tools tests/system` and
`ninja -C build/asan clang-format`. Checks must not rewrite sources. Use the relevant test suites while developing;
[testing](testing.md) describes selection, repetition, fixtures, and final gates.

## GNU C23, standard facilities first

Prefer ISO/IEC 9899:2024 facilities and selected GNU extensions that express intent
clearly. Require the facilities actually used, without older-C fallbacks or C2y syntax.

| Need                               | Convention                                                              |
| ---------------------------------- | ----------------------------------------------------------------------- |
| Boolean/null values and assertions | `bool`, `true`, `false`, `nullptr`, `static_assert`                     |
| Interface annotations              | `[[nodiscard]]`, `[[maybe_unused]]`, `[[fallthrough]]`                  |
| Checked integer arithmetic         | `<stdckdint.h>`: `ckd_add`, `ckd_sub`, `ckd_mul`                        |
| Constants and local inference      | `constexpr`, `auto`, `typeof` when clearer                              |
| Bit operations                     | `<stdbit.h>` when needed                                                |
| Initialization and representation  | `{}`, designated initializers, compound literals, tagged unions         |
| Headers and no-argument functions  | `#pragma once`; `f()` declarations and definitions                      |
| Variadic formatting                | C23 `va_start(args)` and `[[gnu::format(printf, ...)]]`                 |
| Simple scope-owned storage         | `[[gnu::cleanup(function)]]` with an exactly typed, infallible callback |

Use libc for allocation, copying, formatting, and sorting when its contract fits. A
helper should add ownership, bounds, or meaningful errors rather than rename libc.
Implement project-specific GC and segmentation directly; avoid generic allocator,
`defer`, or preprocessor frameworks. Keep roots, launch gates, and transactional OS
cleanup explicit because their ordering matters. Do not use nested-function trampolines,
`void *` arithmetic, packed-layout tricks, or statement-expression frameworks.

### Memory and interface contracts

Use explicit lengths, `size_t` for sizes, fixed-width language integers, and checked
conversions at boundaries. Check aggregate sizes before allocating or indexing; never
request `realloc(pointer, 0)`. Avoid VLAs and unbounded C recursion. Assertions express
internal invariants; malformed input and OS failures require release checks.

Separate mutable owners from borrowed `const` views. State whether an interface copies,
borrows, or consumes data; never cast away `const` to free it. Ordinary C conversions
between object pointers and `void *` need no cast. Numeric narrowing requires a range
check or a documented bound. Initialize allocated structs with typed compound literals;
root only initialized Values. Retained `calloc` pointer arrays rely on the supported
POSIX ABIs' null representation, not an ISO C guarantee.

Do not equate NUL-terminated strings with all language or OS text. Encoded bytes use
`unsigned char`. Empty spans must not pass null pointers to libc functions requiring
valid pointers. Check `snprintf`'s required length before using it as a written count.
Append inputs must not alias growable destination storage. Format strings are trusted.
Fallible results must be handled; an intentional discard needs a local rationale when
it is not evident from the cleanup policy.

Headers compile independently and include the public standard or component declarations
they use. Use forward declarations for incomplete types, not transitive includes.
Include-cleaner findings need review: SDK-internal files do not replace public headers.
Keep feature-test macros in build configuration and actual Linux/macOS differences in
platform code. Conditional branches require an observed supported-platform difference.

## Dependencies and generated data

The sole third-party runtime library is yyjson, private and static through Meson's
`dependency()` and wrap fallback. The wrap owns its version, archive hash, and MIT
notice; its types never cross component interfaces. `force_fallback_for=yyjson` keeps
the dependency instrumented with the selected build. The current release has no upstream
Meson file, so a small overlay declares the library and overrides the dependency.
Review its warning settings on upgrades instead of patching it for unrelated checks.

Track both `data/unicode/` and `src/text/unicode_tables.inc`. The former contains the
pinned official inputs, manifest, license, and independent conformance corpus; the latter
allows ordinary compilation without regeneration. Neither is a cache. Update inputs
explicitly, run `python3 tools/unicode.py`, and verify with `--check`. Ordinary builds
fetch no Unicode data. Keep upstream notices and data comments intact.

Future bundled `stdlib/*.rill` sources are embedded by a Meson build dependency; their
generated C belongs in the build tree. Generate shared metadata only when it eliminates
real duplication. Python tools and tests use the standard library; no Python package is
needed by the installed shell.

## Tool configuration

Start from upstream defaults or a named preset, retaining only project-specific choices.
Meson owns language mode, build type, sanitizers, optimization, LTO, dependency wiring,
and the compile database. Keep default undefined-symbol checking. Add custom options
only for real optional products, such as future fuzz binaries.

`warning_level=3` and `werror=true` establish the compiler baseline. Additional warnings
cover conversions, shadowing, public prototypes, formats, undefined macros, VLAs,
enum switches, qualifiers, alignment, string literals, and floating-point promotion.
Avoid `-Weverything` and flags already covered by these groups. C23 `()` is a prototype;
`-Wstrict-prototypes` adds no useful constraint here. Consult the
[GCC](https://gcc.gnu.org/onlinedocs/gcc/Warning-Options.html) and
[Clang](https://clang.llvm.org/docs/DiagnosticsReference.html) group definitions when
changing diagnostics.

`.clang-format` starts with LLVM style. Meson's include lists restrict formatting and
analysis to project C sources, including untracked files. `.clang-tidy` owns the check
selection: compiler diagnostics, stable analyzer checks, and C-relevant bugprone/CERT
rules. Validate it with `--verify-config` and review `--list-checks` on upgrades. There
is no upstream universal strict-C preset; these are project choices.

Prefer canonical check names to duplicate CERT aliases. Keep both
`bugprone-unused-return-value` and `cert-err33-c`: their default function sets differ.
The analyzer's `DeprecatedOrUnsafeBufferHandling` rule is disabled because it recommends
Annex K replacements unavailable on the target libraries; `bugprone-unsafe-functions`
remains enabled and accounts for availability. Avoid checks that demand redundant
multi-level `void *` casts or unrelated C++ migrations. Local suppressions identify the
check and explain the exception.

Python targets 3.14. Prefer pure transformations, iterators, `pathlib`, context managers,
`argparse`, and `unittest`; keep I/O and process ownership explicit. Local mutable state
is appropriate for buffering and cleanup. `pyproject.toml` owns Ruff and ty settings.
Neither tool is imported by the scripts, and tests install or download nothing.

## Comments and API documentation

Write concise English. Boundary headers own API contracts; implementation comments
explain reasoning and invariants. Markdown specifications own language behavior.
Document what names and types cannot express: ownership, lifetime, units, empty inputs,
failure guarantees, effects, and GC or blocking boundaries.

Use [Doxygen blocks](https://www.doxygen.nl/manual/docblocks.html) beside declarations,
with an explicit `@brief` followed by a blank line before details. Each documented C
header has an `@file` block. Use `///<` for short member contracts. Keep one authoritative
comment per declaration; do not repeat signatures with `@fn` or duplicate contracts in
source files. Use `@pre`, `@return`, or complete `@param` lists only when they add clarity.
There is no tag quota.

```c
/**
 * @brief Append bytes to an owned buffer.
 *
 * The source must not alias buffer storage. Empty input is valid.
 * @return True on success; false leaves the payload unchanged.
 */
```

Ordinary `//` or `/* ... */` comments explain such decisions as root retention, signal
masking, or cleanup order. Avoid assignment narration, decorative banners, historical
audit notes, and commented-out code. Private helpers need explanation only where their
names and implementation leave a meaningful question unanswered.

### Doxygen build

With Doxygen available at Meson setup, generate the HTML reference explicitly:

```sh
meson compile -C build/asan api-docs
```

Output is under the build directory's `api/html/`. Meson owns one boundary-header list
and configures `docs/Doxyfile.in`; private representations, tests, generated tables,
and dependencies are excluded. Ordinary compilation does not require Doxygen, but the
quality gate does. Keep default HTML styling and warning checks; `EXTRACT_ALL` would
hide missing documentation and stays off. Warnings fail the target.

Review rendered C23 signatures, links, and contracts as well as the exit status.
Generated configuration and HTML stay in the build tree; published artifacts must not
contain personal paths. The [Doxygen configuration manual](https://www.doxygen.nl/manual/config.html)
defines these options.

## Documentation maintenance

Use **Rill Shell** for the product and `rillsh` for the executable/package. Keep each
contract in its owning document and link to it elsewhere. README introduces the project
and working examples; AGENTS.md provides concise contributor-agent instructions;
status distinguishes implementation from design. Update these when scope changes.

Keep personal paths, account names, hostnames, credentials, environment dumps, and
host-specific container tooling out of repository files and published evidence. Use
relative paths, standard OS paths, or portable placeholders. Versions and hashes are
appropriate validation identifiers. The quality gate checks tracked and non-ignored
untracked files; ignored logs are local artifacts, not publication-ready evidence.

Escape literal pipes even inside Markdown table code spans. Check links, anchors,
fences, tables, and examples. Record actual pass/fail/unavailable results; a written
contract is not evidence that its implementation exists.
