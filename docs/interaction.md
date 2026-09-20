# Interaction

This document defines invocation, editing and presentation. [Platform](platform.md) owns
encoding and terminal capabilities; [architecture](architecture.md) explains the
mechanisms.

## Invocation and session boundaries

Invocation forms are `rillsh`, `rillsh -c SOURCE`, `rillsh FILE [ARG ...]`, and source
from redirected stdin. `--` ends option parsing. Automatic interactive mode requires
terminal stdin and no supplied script or command; `-i` requests it explicitly and
requires a usable controlling terminal. Combining `-i` with FILE or `-c` is an option
error; there is no script-then-prompt mode. The editor reads and writes the controlling
terminal independently of stdout/stderr redirection. Noninteractive operation uses no
editor, prompt, automatic value display, startup config, or history writes.

`args ()` returns FILE arguments as a List of Bytes, excluding the filename; it is empty
in other modes. `-c` accepts one source argument and no positional arguments. `--help`,
`--version`, and `--color=auto|always|never` have fixed meanings. Interactive startup
reads `rillsh/init.rill` under the user XDG config base. `--config FILE` replaces that
path; `--no-config` disables startup loading. The two options conflict. `--config`
cannot accompany FILE or `-c`. A missing default file is normal; an explicit file error
or other startup failure reports a diagnostic and leaves a usable prompt. Do not
discover startup code in ancestor directories or execute code during completion.

Interactive expressions display their results. Unit and declarations display nothing.
Functions display their remaining parameters without invocation. Records and
materialized Lists use bounded tables; nested values use concise summaries. A JobPlan is
described without launching it. The REPL parses the complete submitted entry before
executing any prefix. Runtime effects completed before a later failure remain real.

## Expression submission

The parser classifies the entire buffer as Complete, Incomplete, or Invalid with source
spans. The editor consumes this service; it does not duplicate the grammar with brace
counts. Bare functions and partial applications are Complete; their signatures never
trigger continuation or invocation. Closure headers and bodies remain Incomplete until
closed. Enter at the end submits Complete/Invalid input and inserts a newline for
Incomplete input. Enter inside the buffer inserts a newline. Invalid input is submitted
for a diagnostic instead of collecting lines indefinitely.

Alt-Enter explicitly submits the entire buffer, including incomplete input for a syntax
diagnostic. Ctrl-J inserts a newline; Shift-Tab does so outside the completion menu.
The Alt binding uses the ordinary ESC-prefixed sequence. Help lists these actions and
aliases; no backslash continuation rewrites source text.

Newlines use Reedline's native editing behavior and do not rewrite indentation or string
contents. One complete submitted entry is one history item and one execution boundary.

## Familiar terminal controls

Use one fixed key set built around navigation keys, familiar terminal shortcuts, and a
few aliases. Copy and paste remain terminal-emulator actions. There are no alternate
editing modes or configurable keymaps.

| Action                                        | Default input                                    |
| --------------------------------------------- | ------------------------------------------------ |
| Move one grapheme; delete before/after cursor | Left/Right; Backspace/Delete                     |
| Move within multiline input                   | Up/Down; at the first/last row, navigate history |
| Move to logical line boundaries               | Home/End; Ctrl-A/Ctrl-E aliases                  |
| Insert newline without submission             | Ctrl-J or Shift-Tab                              |
| Submit according to parser state              | Enter at buffer end                              |
| Submit the whole buffer explicitly            | Alt-Enter                                        |
| Complete; navigate candidates                 | Tab; Tab/Shift-Tab in the menu                   |
| Search previous input incrementally           | Ctrl-R                                           |
| Undo; redo                                    | Ctrl-_; Alt-r                                    |
| Delete preceding word                         | Ctrl-W                                           |
| Delete to logical line end                    | Ctrl-K                                           |
| Close completion or history search            | Escape                                           |
| Cancel current entry                          | Ctrl-C                                           |
| Request EOF on empty buffer; delete otherwise | Ctrl-D                                           |
| Suspend the shell while editing               | Ctrl-Z                                           |
| Redraw; show help                             | Ctrl-L; F1                                       |

Word deletion follows Reedline’s native word boundaries; it does not tokenize Rill. A
completion menu consumes Enter to accept a candidate, never to execute it. Escape closes
an overlay without clearing the main buffer. History search explicitly indicates when
no entry matches. Accepting a match returns to editing without executing it.

Ctrl-C discards the unsubmitted entry. Ctrl-D requests `exit 0` only when the buffer is
empty; outstanding jobs follow the execution shutdown contract. Ctrl-Z restores terminal
state and suspends the shell, retaining the buffer for resume. During evaluation,
interrupt/stop behavior follows [execution](execution.md). Ctrl-Z retains its Unix
suspend meaning; undo has a separate binding.

## Paste and input limits

On the rich terminal profile, bracketed paste is one insertion transaction. Its contents
are not interpreted as shortcuts, completion requests, or submit keys. CRLF and lone CR
are normalized to LF at this terminal-input boundary; no such normalization applies to
files, strings, argv, or process data. Other control bytes remain source data; invalid
source receives a diagnostic. Reedline owns their appearance while editing and recalling
history. Rill does not promise a separate escaped editor display with exact cursor
mapping.

Reedline and Crossterm own paste buffering, decoding, undo and key-sequence recognition.
Rill uses their native APIs without patches. Large input can consume memory before
submission validation; there is no incremental paste limit or bounded undo guarantee.

| Resource                        | Policy                                                     |
| ------------------------------- | ---------------------------------------------------------- |
| Submitted source                | Reject entries exceeding 1 MiB before parsing or execution |
| Editable buffer, paste and undo | Reedline's native behavior; no Rill memory ceiling         |
| History                         | At most 10,000 entries; no aggregate byte ceiling          |
| Completion results              | At most 200 candidates and 1 MiB of retained text          |

Oversized submissions do not execute a prefix and do not enter persistent history.
Reedline may retain its latest leading-space exclusion for in-memory recall even when
submission is rejected; this is part of the native editor buffer policy. Plain input
bounds its accumulated source while reading; rich input is checked after submission.
There is no time-based inference that rapidly typed characters are pasted text.

## Completion, highlighting, and responsiveness

Highlighting uses lexer spans without filesystem access or evaluation. Completion uses
published names, value kinds and remaining function parameters. Field completion
inspects materialized Records and nominal payloads; it never calls a function.

Filesystem completion begins on explicit Tab. A short-lived helper process receives a
bounded query and returns bounded candidates. A result expires after 200 ms; the
supervisor cancels/reaps the worker without waiting inside the editor. At most one
worker may remain unreaped, so a slow filesystem cannot accumulate helpers. Reedline
owns menu generations; Rill binds each reply to its complete buffer/cursor origin,
discards superseded replies and replaces the completer between entries. Stale results
must not open a menu, replace newer text or move its cursor.

Insertion must round-trip to the intended value or exactly one command argument,
including spaces, quotes, `$`, wildcard characters, and non-UTF-8 paths. Use this
language's quoting or explicit Bytes/Path expression syntax, not a POSIX-shell quoting
routine. Do not accept arbitrary code suggested by filesystem contents.

While editing, the coordinator handles signals, child state, deadlines and owned I/O.
Reedline owns repaint scheduling. Plain color mode skips highlighting; large input may
also lose highlighting, but submission always uses the complete parser result.

## Text, styles, and output

The editor uses one palette for strings, scalar literals, keywords and pipeline syntax,
with plain text for other tokens. Native hints and diagnostics provide their own styles.
Color has no semantic role and never replaces useful wording; there is no markup parser
or theme language. [Platform](platform.md#terminal-profile-and-color) defines capability
detection and color precedence.

Rill escapes control and bidirectional-formatting characters in diagnostics, completion
labels and automatic value summaries. Raw editor buffers, native history navigation and
hints follow Reedline's rendering behavior; a source-to-display remapping layer is
outside the accepted native-editor scope. Reedline, Ariadne and Rill tables use their
owning libraries' layout services. Explicit byte/text writes and external program output
remain unchanged.

Diagnostics carry an error kind, message, optional source range and cleanup notes. The
CLI renders them; evaluators and codecs never print into a pipeline. Completion
signatures describe remaining parameters, not a static type or effect system. The
language and execution references own accepted arguments and materialization rules.

Messages use concise English and public type/function names, with an actionable
correction when known. Summaries name their units: items, fields or stages.

A Record displays as field/value rows. A nonempty List displays at most 100 rows; record
rows use up to eight field names from the first row when all displayed records contain
them. Other Lists use index/value rows. Tables fit the terminal width, capped at 240
cells, and mark omitted rows or fields; long cells are abbreviated.

A returned Stream displays each non-Unit item as a summary as it arrives, then finishes
cleanup before publication. It does not sample or materialize rows to build a table.
Display is not serialization; explicit output functions preserve their data.

Shell-owned job notifications and relayed output use a coordinated clear/write/redraw
operation that preserves the edit buffer, cursor, and viewport. Direct inherited output
from background programs may still interleave; the shell cannot safely interpret
arbitrary program escape sequences. Ctrl-L restores its own editing area.

## History and recovery

History uses Reedline's native file backend, including its synchronization, multiline
encoding and consecutive-duplicate policy. Submitted nonempty entries, including failed
submissions, are eligible. A leading ASCII space uses Reedline's memory-only exclusion;
only the latest excluded entry is retained for recall.

The backend encodes newlines with the marker `<\n>`. Entries containing that literal
marker are omitted from history with a notice, because saving them would change their
source on reload. Oversized submissions are also omitted. Existing files must use the
backend's native format; Rill maintains no custom history codec or compatibility reader.

Use the private XDG state location and permissions specified in [platform](platform.md).
Unavailable storage reports a notice and keeps in-memory editing usable.

Every controlled editor exit disables enabled terminal modes and restores the saved
attributes. Terminal/job state is owned by the session, not by a GC finalizer or editor
destructor invoked at an unpredictable time. Recheck foreground ownership before
resuming raw input after suspension. A restored prompt must remain usable after
cancellation, resize, job stop/resume, or a program that changed termios.
