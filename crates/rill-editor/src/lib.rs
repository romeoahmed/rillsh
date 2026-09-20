//! Reedline owns editing and repainting; Rill supplies syntax and session snapshots.
pub mod completion;
mod history;
mod keys;
pub mod profile;
use nu_ansi_term::{Color, Style};
use profile::ColorDepth;
pub use reedline::Signal;
use reedline::{
    DefaultHinter, Highlighter, MenuBuilder, Prompt, PromptEditMode, PromptHistorySearch, Reedline,
    StyledText, ValidationResult, Validator,
};
use std::{
    borrow::Cow,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct Editor {
    line: Option<Reedline>,
    control: Control,
    notifications: Notifications,
    color: ColorDepth,
}
impl Editor {
    /// Create an editor with parser-backed continuation and XDG state history.
    /// Unavailable storage produces one renderer-owned notice and keeps in-memory history.
    #[must_use]
    pub fn new(color: ColorDepth, notifications: Notifications) -> Self {
        let directories = xdg::BaseDirectories::with_prefix("rillsh");
        let control = Control::default();
        let history = directories
            .place_state_file("history")
            .and_then(|path| history::open(&path))
            .unwrap_or_else(|error| {
                let _ = notifications.try_print(format!(
                    "rillsh: persistent history unavailable; using in-memory history: {error}"
                ));
                history::SafeHistory::default()
            });
        Self {
            line: Some(
                Reedline::create()
                    .use_bracketed_paste(true)
                    .with_break_signal(Arc::clone(&control.wake))
                    .with_external_printer(notifications.0.clone())
                    .with_history(Box::new(history))
                    .with_history_exclusion_prefix(Some(" ".into()))
                    .with_edit_mode(Box::new(keys::bindings()))
                    .with_menu(reedline::ReedlineMenu::EngineCompleter(Box::new(
                        reedline::ColumnarMenu::default()
                            .with_name("completion")
                            .with_input_mode(reedline::InputMode::FullBuffer),
                    )))
                    .with_validator(Box::new(Syntax))
                    .with_highlighter(Box::new(Highlight(color)))
                    .with_hinter(Box::new(
                        DefaultHinter::default().with_style(Style::new().dimmed()),
                    ))
                    .with_ansi_colors(color.enabled()),
            ),
            control,
            notifications,
            color,
        }
    }
    /// Apply the current session's color hints without replacing history or editing state.
    pub fn set_color(&mut self, color: ColorDepth) {
        if color != self.color {
            self.line = self.line.take().map(|line| {
                line.with_highlighter(Box::new(Highlight(color)))
                    .with_ansi_colors(color.enabled())
            });
            self.color = color;
        }
    }
    /// Start an entry with a fresh metadata channel, discarding earlier completion results.
    pub fn complete_with(&mut self, sender: tokio::sync::mpsc::Sender<completion::Request>) {
        self.line = self
            .line
            .take()
            .map(|line| line.with_completer(Box::new(completion::Completion::new(sender))));
    }
    /// Send notifications through Reedline's repaint-aware, bounded channel.
    #[must_use]
    pub fn notifications(&self) -> Notifications {
        self.notifications.clone()
    }
    /// Read one complete entry; Reedline finishes the display line for Ctrl+C and Ctrl+D.
    ///
    /// # Errors
    /// Reports terminal input or rendering failure.
    pub fn read(&mut self) -> io::Result<Signal> {
        let mut line = self
            .line
            .take()
            .ok_or_else(|| io::Error::other("editor state is already in use"))?;
        let result = match line.read_line(&RillPrompt(self.color)) {
            Ok(Signal::ExternalBreak(_)) => {
                if !self.control.discard.swap(false, Ordering::Relaxed) {
                    self.line = Some(line);
                    return Ok(Signal::HostCommand("suspend".into()));
                }
                line.run_edit_commands(&[reedline::EditCommand::Clear]);
                line = line.with_immediately_accept(true);
                let result = line.read_line(&RillPrompt(self.color));
                line = line.with_immediately_accept(false);
                result.map(|_| Signal::CtrlC)
            }
            result => result,
        };
        let result = match result {
            Ok(Signal::Success(source)) if source.len() > history::ENTRY_LIMIT => {
                Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "entry exceeds the 1 MiB source limit",
                ))
            }
            Ok(Signal::Success(source)) => {
                if !history::storable(&source) {
                    let _ = self.notifications.try_print("rillsh: entry omitted from history because its text cannot round-trip through the history format".into());
                }
                Ok(Signal::Success(source))
            }
            result => result,
        };
        self.line = Some(line);
        result
    }
    /// A signal-only handle; it never accesses the editor buffer from another thread.
    #[must_use]
    pub fn control(&self) -> Control {
        self.control.clone()
    }
}
/// Signal-only editor control; the coordinator never accesses its mutable buffer.
#[derive(Clone, Default)]
pub struct Control {
    wake: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
}
impl Control {
    /// Interrupt the active read, discarding the entry on cancellation only.
    pub fn interrupt(&self, discard: bool) {
        self.discard.store(discard, Ordering::Relaxed);
        self.wake.store(true, Ordering::Release);
    }
}
/// A nonblocking notification endpoint; it never writes over the active prompt.
#[derive(Clone, Default)]
pub struct Notifications(reedline::ExternalPrinter<String>);
impl Notifications {
    /// Drain queued messages at a plain-input prompt boundary.
    pub fn drain(&self) -> impl Iterator<Item = String> + '_ {
        self.0.receiver().try_iter()
    }
    /// Queue one message. A full queue leaves retry policy with the coordinator.
    #[must_use]
    pub fn try_print(&self, message: String) -> bool {
        self.0.sender().try_send(message).is_ok()
    }
}

/// Resolve the user startup path without searching system directories or creating files.
/// The caller opens it and treats a missing file as an absent optional configuration.
#[must_use]
pub fn startup_file() -> Option<PathBuf> {
    xdg::BaseDirectories::with_prefix("rillsh").get_config_file("init.rill")
}
struct Syntax;
impl Validator for Syntax {
    fn validate(&self, line: &str) -> ValidationResult {
        if line.len() > history::ENTRY_LIMIT {
            return ValidationResult::Complete;
        }
        match rill_syntax::parse("<input>", line) {
            Err(errors) if errors.iter().all(|error| error.incomplete) => {
                ValidationResult::Incomplete
            }
            _ => ValidationResult::Complete,
        }
    }
}
struct Highlight(ColorDepth);
impl Highlighter for Highlight {
    fn highlight(&self, line: &str, _: usize) -> StyledText {
        use rill_syntax::token::Kind;
        let mut output = StyledText::new();
        if !self.0.enabled() || line.len() > history::ENTRY_LIMIT {
            output.push((Style::new(), line.into()));
            return output;
        }
        let Ok(tokens) = rill_syntax::token::lex(line) else {
            output.push((Style::new(), line.into()));
            return output;
        };
        let mut position = 0;
        for token in tokens {
            if position != token.span.start {
                output.push((Style::new(), line[position..token.span.start].into()));
            }
            let color = match token.kind {
                Kind::String(_) => self.0.select(Color::Green, 114, (152, 195, 121)),
                Kind::Number(_) | Kind::True | Kind::False | Kind::Null => {
                    self.0.select(Color::Yellow, 173, (209, 154, 102))
                }
                Kind::Let
                | Kind::Fn
                | Kind::Rec
                | Kind::If
                | Kind::Then
                | Kind::Else
                | Kind::Match
                | Kind::Of
                | Kind::Do
                | Kind::With
                | Kind::Job
                | Kind::Import
                | Kind::As
                | Kind::Export
                | Kind::Struct
                | Kind::Enum => self.0.select(Color::Purple, 176, (198, 120, 221)),
                Kind::Command | Kind::Pipe | Kind::ValuePipe | Kind::Redirect(_) => {
                    self.0.select(Color::Cyan, 73, (86, 182, 194))
                }
                _ => {
                    output.push((Style::new(), line[token.span.clone()].into()));
                    position = token.span.end;
                    continue;
                }
            };
            output.push((Style::new().fg(color), line[token.span.clone()].into()));
            position = token.span.end;
        }
        if position != line.len() {
            output.push((Style::new(), line[position..].into()));
        }
        output
    }
}
struct RillPrompt(ColorDepth);
impl Prompt for RillPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        "rill".into()
    }
    fn render_prompt_right(&self) -> Cow<'_, str> {
        "".into()
    }
    fn render_prompt_indicator(&self, _: PromptEditMode) -> Cow<'_, str> {
        "> ".into()
    }
    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        "... ".into()
    }
    fn render_prompt_history_search_indicator(&self, search: PromptHistorySearch) -> Cow<'_, str> {
        let label = match search.status {
            reedline::PromptHistorySearchStatus::Passing => "history",
            reedline::PromptHistorySearchStatus::Failing => "history (no match)",
        };
        format!("{label} '{}': ", search.term.escape_debug()).into()
    }
    fn get_prompt_color(&self) -> Color {
        self.0.select(Color::Blue, 75, (97, 175, 239))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlighting_preserves_source_in_every_color_mode() {
        for depth in [
            ColorDepth::Plain,
            ColorDepth::Ansi16,
            ColorDepth::Ansi256,
            ColorDepth::TrueColor,
        ] {
            for source in [
                "",
                "  ",
                "let x = \"\u{754c}\"\n# comment\nx |> text",
                "^printf '%s' $x",
                "\"unfinished",
            ] {
                let styled = Highlight(depth).highlight(source, source.len());
                assert_eq!(styled.raw_string(), source);
                if depth == ColorDepth::Plain {
                    assert_eq!(styled.render_simple(), source);
                }
            }
        }
    }

    #[test]
    fn history_search_reports_no_match_and_escapes_the_query() {
        let prompt = RillPrompt(ColorDepth::Plain);
        let search = PromptHistorySearch::new(
            reedline::PromptHistorySearchStatus::Failing,
            "missing\n\u{1b}".into(),
        );
        let rendered = prompt.render_prompt_history_search_indicator(search);
        assert!(rendered.contains("no match"));
        assert!(!rendered.chars().any(char::is_control));
    }

    #[test]
    fn continuation_uses_language_syntax() {
        for source in ["{ x =>", "let value =", "^printf $("] {
            assert!(
                matches!(Syntax.validate(source), ValidationResult::Incomplete),
                "{source}"
            );
        }
        for source in ["map", "let value = 1", "1 + )"] {
            assert!(
                matches!(Syntax.validate(source), ValidationResult::Complete),
                "{source}"
            );
        }
    }
}
