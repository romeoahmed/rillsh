# Platform

This document defines Linux/macOS support, native bytes, storage and terminal policy.
[Execution](execution.md) specifies job semantics; [architecture](architecture.md)
specifies process and terminal mechanisms. Rust libraries own Unicode algorithms and
native backend selection; no custom Unicode database or obsolete terminal dialect is
maintained.

## Standards and support boundary

Target current Linux/glibc and macOS environments using POSIX.1-2024 terminology and
semantics. Require the facilities in use, not full platform certification. Linux/macOS
API differences belong in the platform adapter; missing facilities are build errors.

| Area                          | Baseline                                 | Project boundary                                             |
| ----------------------------- | ---------------------------------------- | ------------------------------------------------------------ |
| Implementation language       | Current stable Rust, Edition 2024        | Cargo workspace; generated C only for Tree-sitter            |
| Processes and system services | POSIX.1-2024 / Issue 8                   | Linux/macOS user-space APIs; no POSIX shell grammar claim    |
| User directories              | XDG Base Directory 0.8                   | Same config/state policy on both platforms                   |
| Text and segmentation         | UTF-8 and supported library Unicode data | Extended grapheme editing; byte-preserving native paths      |
| JSON                          | RFC 8259                                 | Stricter duplicate-key and numeric-range policy in execution |
| Terminal UI                   | Modern VT/xterm-compatible UTF-8 profile | SGR, cursor control, bracketed paste; plain fallback         |

`termios` controls a terminal device; `terminfo` describes capabilities. The shell uses
the former and a fixed modern protocol profile, with no terminfo dependency or
replacement database.

## Unix processes and executables

Use process groups, sessions, descriptors, pipes, exec, wait, signals, and termios
according to their native contracts. `poll`, monotonic clocks, `sigaction`, and
`waitpid` are shared services. The terminal adapter contains the system's window-size
query and `/dev/tty` access. These services stay behind the system crate’s ownership
boundary.

Tokio supplies asynchronous readiness for supported descriptors. Canonical terminal and
inherited-stdin reads use a cancellable POSIX poll worker, joined before input handoff.
Do not impose a fixed descriptor-number ceiling. Readiness errors remain errors;
interruption and EOF are distinct outcomes.

Use owned descriptors and native cleanup; never retry an ordinary close after
ownership ends. rustix selects its Linux raw backend on supported Linux targets and
libc on macOS. Do not force a workspace-wide backend or add a second close protocol.

Do not create a new session per command. Interactive terminal jobs receive the
foreground process group; background jobs remain subject to terminal access rules.
Noninteractive execution does not acquire interactive terminal control, although managed
children may have private groups for cancellation. Launch and restoration ordering
belongs to [architecture](architecture.md#process-launch-and-io).

Tokio owns signal delivery to the coordinator. The fresh child helper restores the
required default dispositions and mask before executing a target. Terminal ownership,
stop/resume and launch barriers are explicit system contracts, not assumptions about
`std::process::Command` defaults.

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
update affects future child snapshots. It does not change numeric syntax. Terminal hints
are reevaluated at the next prompt; config/history locations are resolved once when the
session starts.

HOME and XDG variables locate user files. `cd path` is explicit: no CDPATH, implicit
home argument, or tilde expansion. After successful `cd`, maintain physical absolute PWD
and OLDPWD bytes. Validate the destination and prepare its physical name before swapping
the owned cwd capability. The coordinator never changes process-global cwd, so failed
preparation leaves its directory and environment unchanged. If the old directory no
longer has a name, remove OLDPWD rather than inventing one. At startup, an unavailable
physical cwd leaves PWD unset.

Preserve SHELL as the inherited preferred-shell setting. Do not import exported
functions, IFS behavior, SHLVL semantics, or `_` updates from other shells. Inherit
umask for ordinary filesystem effects; private state uses explicit restrictive modes.
LANG, LC_ALL, and LC_* pass unchanged to children for their own locale policy.

## XDG storage

The xdg crate resolves the user's configuration and state directories. Rill adds the
`rillsh` application prefix and uses two files:

| Purpose | Default location                                                                  | Selection                                                                |
| ------- | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Startup | `$XDG_CONFIG_HOME/rillsh/init.rill` (`$HOME/.config/rillsh/init.rill` by default) | Interactive only; `--config FILE` replaces it, `--no-config` disables it |
| History | `$XDG_STATE_HOME/rillsh/history` (`$HOME/.local/state/rillsh/history` by default) | Reedline's native file backend                                           |

Directory defaults and HOME resolution belong to xdg; Rill has no second fallback or
search policy. XDG_CONFIG_DIRS and XDG_DATA_DIRS remain child environment variables, not
startup-code or module search paths. No cache or runtime directory is reserved.

History uses mode 0700 for the application state directory and 0600 for its file,
without changing user-managed base directories. Startup and unavailable-history behavior
are defined in [interaction](interaction.md).

## UTF-8, graphemes, and display width

Byte offsets, scalar values, extended grapheme clusters, and terminal cells are distinct
units. Source and String decoding reject overlong UTF-8, surrogates, out-of-range code
points, and malformed sequences. Incremental decoders retain incomplete sequences across
reads. Bytes and Path preserve non-UTF-8 OS data. Canonical input validates UTF-8
explicitly; rich input uses Crossterm's native decoding.

Identifiers remain ASCII. Text equality/order uses scalar sequences without implicit
normalization or locale collation. Numeric syntax, JSON and formatting are locale
independent. LANG, LC_ALL and LC_* pass unchanged to children. macOS filesystem behavior
does not authorize normalization of Path bytes; actual filesystem support for invalid
UTF-8 filenames can differ from the shell's byte-preserving argument contract.

Reedline owns grapheme editing; unicode-segmentation and unicode-width serve Rill tables;
Ariadne owns diagnostic layout. Cargo.lock records their versions. Accept upstream
Unicode updates without private tables or a separately pinned Unicode release. Fonts
and emoji presentation may disagree with width estimates. Full bidirectional editing is
outside scope.

## Terminal profile and color

Rich editing requires a VT-compatible terminal, usable dimensions and standard streams
attached to the same terminal. Reedline owns cursor movement, line erasure and bracketed
paste. Rill does not maintain a terminal-brand allowlist or probe terminal identity.

Missing/empty TERM, `TERM=dumb`, or unusable dimensions select plain interaction:
canonical input and parser-driven continuation prompts, without cursor-addressed
editing or completion UI. On EOF, incomplete accumulated source is diagnosed; never
execute a valid prefix of an incomplete entry. Both modes use the same parser and
evaluator.

Do not link terminfo/ncurses, run `tput`, parse terminal databases, or negotiate
mouse/clipboard/enhanced keyboard protocols. No terminal-identity or background-color
query is needed. Use kernel dimensions and the editor library's native input decoder.
Resize and Ctrl-L request a redraw.

Automatic color is per output destination and requires a TTY, nonempty TERM other than
`dumb`, and no nonempty NO_COLOR. A terminal with `COLORTERM=truecolor`/`24bit` or a `-direct`
TERM uses RGB; `-256color` uses 256 colors; other terminals use 16. Do not infer
RGB from `xterm-256color`. Map semantic RGB styles to the selected palette centrally.

`--color=never` disables generated styles. `--color=always` overrides TTY/NO_COLOR
suppression and permits SGR, using 16 colors when no stronger capability is known; an
explicit truecolor COLORTERM hint permits RGB in this mode. A color override never
enables cursor controls. External program bytes are unchanged.

Bracketed-paste mode is enabled while rich input is active and disabled before handoff,
suspension, or controlled exit. Raw input mode, signals, and restoration have one
session owner. Uncatchable SIGKILL/SIGSTOP cannot carry a cleanup guarantee; normal
termination and cancellation must restore the appropriate saved state.
