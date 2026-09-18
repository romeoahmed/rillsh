# Platform

This document defines the target Linux/macOS boundary and first-release conventions.
[Execution](execution.md) specifies job semantics; [architecture](architecture.md)
specifies process and terminal mechanisms. [Current status](status.md) records verified
support. Configuration, process services, UTF-8 segmentation, and color selection are
implemented. History storage, grapheme editing, full display width, and rich-terminal
selection remain stage-4 requirements.

## Standards and support boundary

Use published standards and current supported Linux/glibc and macOS development
environments. New code follows POSIX.1-2024 terminology and semantics, requiring the
facilities it uses rather than full platform certification. Missing required facilities
are build errors; Linux/macOS API differences belong in the platform adapter. Older
compiler modes, obsolete terminal dialects, and speculative ports are outside scope.

| Area | Baseline | Project boundary |
| --- | --- | --- |
| Implementation language | GNU C23, based on ISO/IEC 9899:2024 | Standard facilities first, selected GCC/Clang extensions; no older-C fallback or C2y dependency |
| Processes and system services | POSIX.1-2024 / Issue 8 | Linux/macOS user-space APIs; no POSIX shell grammar claim |
| User directories | XDG Base Directory 0.8 | Same config/state policy on both platforms |
| Text and segmentation | Unicode 18.0.0; matching UAX #29 and data | UTF-8 language text; extended grapheme editing |
| JSON | RFC 8259 | Stricter duplicate-key and numeric-range policy in execution |
| Terminal UI | Modern VT/xterm-compatible UTF-8 profile | SGR, cursor control, bracketed paste; plain fallback |

`termios` controls a terminal device; `terminfo` describes capabilities. The shell uses
the former and a fixed modern protocol profile, with no terminfo dependency or
replacement database.

## Unix processes and executables

Use process groups, sessions, descriptors, pipes, exec, wait, signals, and termios
according to their native contracts. `poll`, monotonic clocks, `sigaction`, and
`waitpid` are shared services. The terminal adapter contains the system's window-size
query and `/dev/tty` access. These are platform services rather than ISO C facilities.

Input readiness must not impose a fixed descriptor-number ceiling. Linux uses `poll`;
macOS retains `select` for `/dev/tty`, with a bitmap sized to the descriptor under
`_DARWIN_C_SOURCE`. Ordinary descriptor numbers use stack storage; see Apple's [select
contract](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/select.2.html).

Owned descriptor cleanup calls `close` once and invalidates the stored descriptor. Linux
and Darwin release ordinary descriptors even when close reports EINTR; retrying can
close an unrelated descriptor that reused the number. This deliberately follows [Linux
behavior](https://man7.org/linux/man-pages/man2/close.2.html) and [Darwin
implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_descrip.c),
which differ from the POSIX.1-2024 EINTR rule.

Do not create a new session per command. Interactive terminal jobs receive the
foreground process group; background jobs remain subject to terminal access rules.
Noninteractive execution does not acquire interactive terminal control, although managed
children may have private groups for cancellation. Launch and restoration ordering
belongs to [architecture](architecture.md#process-launch-and-io).

Initialization saves the inherited signal mask and dispositions, installs handlers and
their wakeup pipe, then unblocks the signals owned by the session. Unrelated mask bits
remain unchanged. Cleanup and failed initialization restore the inherited state;
children reset their mask and the shell's dispositions before exec. An inherited mask is
not implicitly cleared by
[exec](https://pubs.opengroup.org/onlinepubs/9799919799/functions/exec.html).

Script, configuration, module, and redirection opens use `O_NOCTTY` to prevent
accidental terminal acquisition. File modules must be regular files; opening with
`O_NONBLOCK` allows type validation to reject a FIFO without waiting for a writer. These
flags use the native [open
contract](https://pubs.opengroup.org/onlinepubs/9799919799/functions/open.html).

Executable lookup uses the launch environment's PATH. A slash bypasses lookup; missing
PATH uses `/usr/bin:/bin`. An empty component explicitly means the launch cwd, including
an entirely empty PATH. Continue past absent and permission-denied candidates; report
permission denied if no usable candidate exists but one was denied. Other substantive
errors remain errors. Do not preflight with `access` and assume permissions cannot
change before `execve`.

Pass argv exactly as constructed; the shell does not insert `--` into external
arguments. Internal FDs are close-on-exec. Inherit ordinary standard streams unless the
execution function or plan specifies otherwise. Shebang handling belongs to the OS;
examples use `#!/usr/bin/env rillsh` without multiple shebang arguments. A text file
with an unsupported executable format fails with ENOEXEC and never falls back to another
shell interpreter. Explicit script invocation still works normally.

Installation provides `rillsh`. Automatic `/etc/shells` registration, `/bin/sh`
replacement, login-shell configuration, and privileged/set-ID execution are outside
scope. Do not read `.profile`, `.bashrc`, `.inputrc`, or `.editrc` implicitly.

## Environment conventions

Maintain an explicit session environment map separate from lexical bindings and snapshot
it per launch. Preserve unknown inherited variables and arbitrary non-NUL value bytes.
Split inherited entries at the first `=`; ignore malformed entries without a nonempty
name and keep the first duplicate. This duplicate rule is project policy, since portable
behavior for duplicate environment names is not defined. Undecodable names remain
preservable without becoming language identifiers.

`get_env`, `set_env`, and `unset_env` are explicit effects; unset differs from empty. An
update affects future child snapshots. It does not reinitialize C locale or change
numeric syntax. Terminal hints are reevaluated at the next prompt; config/history
locations are resolved once when the session starts.

HOME and XDG variables locate user files. `cd(path)` is explicit: no CDPATH, implicit
home argument, or tilde expansion. After successful `cd`, maintain physical absolute PWD
and OLDPWD bytes. Prepare bookkeeping and retain a directory FD for rollback. If
preparation after chdir fails, restore the old directory and environment. If the OS also
rejects rollback, report both failures and invalidate PWD rather than claiming the old
state was preserved. At startup, an unavailable cwd likewise leaves PWD unset.

Preserve SHELL as the inherited preferred-shell setting. Do not import exported
functions, IFS behavior, SHLVL semantics, or `_` updates from other shells. Inherit
umask for ordinary filesystem effects; private state uses explicit restrictive modes.
LANG, LC_ALL, and LC_* pass unchanged to children for their own locale policy.

## XDG storage

| Purpose | Base variable | Default | Use |
| --- | --- | --- | --- |
| Configuration | `XDG_CONFIG_HOME` | `$HOME/.config` | `rillsh/init.rill`, interactive only |
| Persistent state | `XDG_STATE_HOME` | `$HOME/.local/state` | `rillsh/history` and separate lock file |
| Disposable cache | `XDG_CACHE_HOME` | `$HOME/.cache` | Reserved; completion cache is in memory |
| User data | `XDG_DATA_HOME` | `$HOME/.local/share` | Reserved; no implicit module discovery |
| Session files | `XDG_RUNTIME_DIR` | No general default | Not needed; helper communication uses pipes |

An unset, empty, or relative base value selects its default. If that default needs HOME
and HOME is not absolute/nonempty, disable the optional facility with one diagnostic. A
valid explicit absolute XDG base works without HOME. Never fall back to the cwd or
create unused directories. XDG_CONFIG_DIRS and XDG_DATA_DIRS are not startup-code or
module search paths for this shell; preserve them for children.

Create the project state directory with mode 0700 and its files with mode 0600, without
changing user-managed base-directory permissions. Unwritable state disables persistence,
not the REPL. History locking and replacement belong to interaction.

## UTF-8, graphemes, and display width

Byte offsets, scalar values, extended grapheme clusters, and terminal cells are distinct
units. Source and String decoding reject overlong UTF-8, surrogates, out-of-range code
points, and malformed sequences. Incremental decoders retain incomplete sequences across
reads. Bytes and Path preserve non-UTF-8 OS data. Invalid terminal input is reported
without changing previously accepted text; it is not silently replaced.

Identifiers remain ASCII. Text equality/order uses scalar sequences without implicit
normalization or locale collation. macOS filesystem behavior does not authorize
rewriting path bytes. Numeric syntax, JSON, and language formatting use `.` and stable
ASCII rules. Keep locale-independent language operations independent of
`setlocale(LC_ALL, "")`; the initial runtime leaves the C locale in place and uses
explicit UTF-8 services. It does not force a locale into the child's environment.

Editor Left/Right and deletion operate on default extended grapheme clusters under
Unicode 18.0.0 UAX #29. Generate property tables from that release's UCD, including
Grapheme_Cluster_Break, Extended_Pictographic, and Indic_Conjunct_Break; run its
official GraphemeBreakTest corpus. Do not copy handwritten ranges or invent emoji
adjacency rules. Language indexing and comparison are not implicitly changed by editor
behavior.

A separate, explicit display policy uses East_Asian_Width, combining/default-ignorable
properties, and the same release's emoji sequence/presentation data. Ordinary text
clusters use the maximum visible constituent width (wide/fullwidth: 2; ambiguous and
other printable text: 1); combining/default-ignorable constituents add no width.
Recognized emoji-presentation clusters use 2 cells, with variation selectors resolved
before the fallback text rule. A standalone cluster with no visible base gets a visible
placeholder of known width. Newlines, tabs, escaped controls, and placeholders are
layout operations rather than characters passed to this width formula.

This is a deterministic terminal policy, not a claim that UAX #11 defines terminal width
or every terminal/font agrees. Do not combine platform `wcwidth` results with another
table in one layout. Source is edited in logical order; full bidirectional reordering is
outside scope. Expose invisible directional controls in source diagnostics.
Unknown/newly assigned glyphs remain valid text even when terminal width is imperfect.

Use one Unicode version for properties, algorithms, and test corpora. Data generation
and licensing follow [development](development.md#dependencies-and-generated-data).

## Terminal profile and color

The rich profile requires a UTF-8 VT/xterm-compatible terminal, usable dimensions,
relative cursor movement, line erasure, SGR, and bracketed paste. Test common macOS and
Linux emulators and tmux/screen passthrough. TERM is a capability hint, not proof that a
brand implements every extension. Known families include `xterm*`, `screen*`, `tmux*`,
`rxvt*`, `foot*`, `kitty*`, and `alacritty*`.

Missing/empty TERM, `TERM=dumb`, unknown/legacy profiles, or unusable dimensions select
plain interaction: canonical input, parser-driven continuation prompts, no
cursor-addressed editing, completion UI, or paste guarantee. No legacy console escape
implementation is required. On EOF, incomplete accumulated source is diagnosed; never
execute a valid prefix of an incomplete entry. Plain and rich input feed the same parser
and evaluator.

Do not link terminfo/ncurses, run `tput`, parse terminal databases, or negotiate
mouse/clipboard/enhanced keyboard protocols. No terminal-identity or background-color
query is needed. Use kernel dimensions; decoder deadlines keep partial keys from
blocking the event loop. Resize and Ctrl-L invalidate layout and redraw safely.

Automatic color is per output destination and requires a TTY, a known profile, and no
nonempty NO_COLOR. A known profile with `COLORTERM=truecolor`/`24bit` or a `-direct`
TERM uses RGB; `-256color` uses 256 colors; other known profiles use 16. Do not infer
RGB from `xterm-256color`. Map semantic RGB styles to the selected palette centrally.

`--color=never` disables generated styles. `--color=always` overrides TTY/NO_COLOR
suppression and permits SGR, using 16 colors when no stronger capability is known; an
explicit truecolor COLORTERM hint permits RGB in this mode. A color override never
enables cursor controls. External program bytes are unchanged.

Bracketed-paste mode is enabled while rich input is active and disabled before handoff,
suspension, or controlled exit. Raw input mode, signals, and restoration have one
session owner. Uncatchable SIGKILL/SIGSTOP cannot carry a cleanup guarantee; normal
termination and cancellation must restore the appropriate saved state.
