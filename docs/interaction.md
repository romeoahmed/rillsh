# Interaction

This document specifies the first-release target for invocation and interaction.
[Architecture](architecture.md) defines editor mechanisms; [platform](platform.md)
defines encoding and terminal capabilities. [Current status](status.md) describes the
canonical-input REPL available today. Invocation is implemented; the editing,
completion, history, and rich presentation contracts below belong to stage 4.

## Invocation and session boundaries

Invocation forms are `rillsh`, `rillsh -c SOURCE`, `rillsh FILE [ARG ...]`, and source
from redirected stdin. `--` ends option parsing. Automatic interactive mode requires
terminal stdin and no supplied script or command; `-i` requests it explicitly and
requires a usable controlling terminal. Combining `-i` with FILE or `-c` is an option
error; there is no script-then-prompt mode. The editor reads and writes the controlling
terminal independently of stdout/stderr redirection. Noninteractive operation uses no
editor, prompt, automatic value display, startup config, or history writes.

`args()` returns FILE arguments as a List of Bytes, excluding the filename; it is empty
in other modes. `-c` accepts one source argument and no positional arguments. `--help`,
`--version`, `--no-config`, and `--color=auto|always|never` have fixed meanings. The
interactive startup file is `rillsh/init.rill` under the resolved XDG config base.
Startup-file failure reports a diagnostic and leaves a usable prompt. Do not discover
startup code in ancestor directories or execute code during completion.

Interactive expressions display their results. Unit and declarations display nothing;
Function display shows available signature metadata without invocation. A JobPlan is
described without launching it. The REPL parses the complete submitted entry before
executing any prefix. Runtime effects completed before a later failure remain real.

## Expression submission

The parser classifies the entire buffer as Complete, Incomplete, or Invalid with source
spans. The editor consumes this service; it does not duplicate the grammar with brace
counts. Enter at the end submits Complete/Invalid input and inserts a newline for
Incomplete input. Enter inside the buffer inserts a newline. Invalid input is submitted
for a diagnostic instead of collecting lines indefinitely.

Alt-Enter explicitly submits the entire buffer, including incomplete input for a syntax
diagnostic. Ctrl-J or Shift-Tab always inserts a newline. The Alt binding uses the
ordinary ESC-prefixed sequence; terminals need not support distinct Ctrl/Shift-Enter
codes. The help view lists these actions and their available aliases. No backslash-based
editor continuation rewrites source text.

Indentation after a newly inserted line uses parser context and a two-space step. It
only adds whitespace at the insertion point. Inside a multiline string, insert only the
requested newline: indentation and string contents must not be rewritten. Highlight
matching delimiters; do not silently insert/correct braces or quotes. One complete
submitted entry is one history item and one execution boundary.

## Familiar terminal controls

Use one fixed key set built around navigation keys, familiar terminal shortcuts, and a
few aliases. Copy and paste remain available through the terminal emulator. Bindings
require neither Command-key nor Ctrl-Shift-key delivery. Editing modes and configurable
keymaps are outside the first release.

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

Word deletion uses runs of whitespace, identifier characters, or punctuation; it is an
editing convenience rather than a second language tokenizer. A completion menu consumes
Enter to accept a candidate, never to execute it. Escape closes an overlay without
clearing the main buffer. Search acceptance returns to editing; it does not execute the
selected history entry.

Ctrl-C discards the unsubmitted entry. Ctrl-D requests `exit(0)` only when the buffer is
empty; outstanding jobs follow the execution shutdown contract. Ctrl-Z restores terminal
state and suspends the shell, retaining the buffer for resume. During evaluation,
interrupt/stop behavior follows [execution](execution.md). Ctrl-Z retains its Unix
suspend meaning; undo has a separate binding.

## Paste and input limits

On the rich terminal profile, bracketed paste is one insertion transaction. Its contents
are not interpreted as shortcuts, completion requests, or submit keys. CRLF and lone CR
are normalized to LF at this terminal-input boundary; no such normalization applies to
files, strings, argv, or process data. Other control bytes remain literal data and are
displayed safely; invalid source receives a diagnostic.

Buffer a paste incrementally with a fixed limit, service other events between chunks,
and commit only after its closing marker and UTF-8 validation. On invalid UTF-8 or
overflow, discard that paste and drain through its closing marker; do not interpret its
remainder as commands. An externally delivered interrupt may abort editing, but input
remains in discard-until-marker state before accepting new keys. EOF or terminal loss
cancels the pending paste. There is no time-based inference that rapidly typed
characters must be pasted text.

| Resource                                       | First-release limit                            |
| ---------------------------------------------- | ---------------------------------------------- |
| Editable entry, including an uncommitted paste | 1 MiB of UTF-8 bytes                           |
| Undo/redo log                                  | 1,000 transactions and 8 MiB of retained edits |
| Persistent history                             | 10,000 entries and 16 MiB                      |
| Completion list                                | 200 candidates and 1 MiB of retained text      |
| Key-sequence recognition                       | 64 bytes; 100 ms ESC/key ambiguity deadline    |

Over-limit edits leave the previous buffer unchanged and produce a concise message. Old
undo/history entries may be evicted according to their stated bounds. Consecutive text
insertion forms an undo group until navigation or another action; a paste or completion
replacement is one group. Paste and discarded control-string payloads are not key
sequences; their terminators, not this deadline, delimit draining. Plain terminals have
reduced editing and no bracketed-paste guarantee, as documented in platform.

## Completion, highlighting, and responsiveness

Highlighting uses lexical/parser spans and in-memory metadata, with no filesystem access
or user evaluation. Functions, constructors, module exports, and builtin signatures
supply help and identifier completion; field completion inspects only materialized data
and descriptors.

Filesystem completion begins on explicit Tab. A short-lived helper process receives a
bounded query and returns bounded candidates. A result expires after 200 ms; the
supervisor cancels/reaps the worker without waiting inside the editor. At most one
worker may remain unreaped, so a slow filesystem cannot accumulate helpers. Requests
carry the buffer revision and completion generation; cursor movement, cancellation, or
another request invalidates the generation even if the text did not change. Stale
results cannot open a menu, replace text, or move the cursor.

Insertion must round-trip to the intended value or exactly one command argument,
including spaces, quotes, `$`, wildcard characters, and non-UTF-8 paths. Use this
language's quoting or explicit Bytes/Path expression syntax, not a POSIX-shell quoting
routine. Do not accept arbitrary code suggested by filesystem contents.

While editing, the event loop continues handling signals, child state, deadlines, and
owned I/O. Coalesce redraws rather than repainting once per input byte. Large input may
temporarily lose highlighting; submission always uses the complete parser result. At a
valid revision, syntax coloring must not disagree with tokenization.

## Text, styles, and output

Content and style are separate data. Semantic roles include keyword, function,
constructor, string, number, path, operator, comment, suggestion, error, warning, and
hint. One default RGB palette with accessible plain wording is sufficient; there is no
markup parser or user theme language. Terminal capability and color precedence belong to
platform and apply consistently to editor and diagnostics.

Source snippets, filenames, history, and values are literal spans. Escape terminal
controls in shell-generated display; never splice untrusted ANSI/OSC sequences into
output. Use one width/layout service for the editor, diagnostics, and tables. External
bytes deliberately written by a program remain unchanged.

Diagnostics carry kind, source name, byte span, message, and optional notes/help. No
evaluator or codec prints directly into a pipeline. Builtin metadata supplies parameter
names, accepted value categories, effect class, materialization behavior, and help; it
does not create a separate function/type system.

The renderer can display records and materialized lists as tables. A returned stream is
drained before publication and displayed as items arrive. If shown as a table, the first
row and terminal width determine columns; later cells may be truncated. Formatting never
pulls extra items merely to sample widths. Heterogeneous rows use ordinary value
presentation. Display is not serialization for process input or files.

Shell-owned job notifications and relayed output use a coordinated clear/write/redraw
operation that preserves the edit buffer, cursor, and viewport. Direct inherited output
from background programs may still interleave; the shell cannot safely interpret
arbitrary program escape sequences. Ctrl-L restores its own editing area.

## History and recovery

History stores submitted nonempty entries, including failed submissions. An entry
beginning with ASCII space remains in memory only. Preserve complete multiline text in a
length-aware, versioned format; reject malformed or over-limit stored entries.

Private modes, path resolution, and failure behavior are specified in platform. Use a
separate advisory-lock file for read/merge/atomic-replace updates, so replacement of the
history inode cannot bypass synchronization. Under the lock, reload current history,
append only this session's unpersisted entries, and retain the newest entries within the
limits. Storage failure reports once and leaves in-memory editing usable.

Every controlled editor exit disables enabled terminal modes and restores the saved
attributes. Terminal/job state is owned by the session, not by a GC finalizer or editor
destructor invoked at an unpredictable time. Recheck foreground ownership before
resuming raw input after suspension. A restored prompt must remain usable after
cancellation, resize, job stop/resume, or a program that changed termios.
