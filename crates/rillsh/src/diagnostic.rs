//! Escape terminal controls before rendering while preserving the source label's byte range.
use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};
use rill_editor::profile::ColorDepth;
use std::{borrow::Cow, fmt::Display, ops::Range};

/// Print a source-free diagnostic with terminal controls escaped.
pub fn message(message: impl Display) {
    eprintln!("rillsh: {}", text(&message.to_string()));
}

pub fn report(error: &rill_runtime::Error, color: ColorDepth) {
    if let Some(source) = &error.origin {
        render(
            &source.name,
            &source.text,
            error.span.clone().unwrap_or(0..0),
            &error.kind,
            &error.message,
            color,
        );
    } else {
        message(error);
    }
    for note in &error.notes {
        eprintln!("  note: {}", text(note));
    }
}

pub fn render(
    name: &str,
    source: &str,
    span: Range<usize>,
    kind: &str,
    message: &str,
    color: ColorDepth,
) {
    let name = text(name);
    let (source, span) = context(source, span);
    let _ = Report::build(ReportKind::Error, (name.as_ref(), span.clone()))
        .with_config(
            Config::default()
                .with_index_type(IndexType::Byte)
                .with_color(color.enabled()),
        )
        .with_message(text(kind))
        .with_label(Label::new((name.as_ref(), span)).with_message(text(message)))
        .finish()
        .eprint((name.as_ref(), Source::from(source)));
}

const fn unsafe_display(character: char) -> bool {
    character.is_control()
        || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub fn text(source: &str) -> Cow<'_, str> {
    if !source.chars().any(unsafe_display) {
        return Cow::Borrowed(source);
    }
    let mut output = String::new();
    for character in source.chars() {
        if unsafe_display(character) {
            output.extend(character.escape_debug());
        } else {
            output.push(character);
        }
    }
    Cow::Owned(output)
}

fn context(source: &str, span: Range<usize>) -> (Cow<'_, str>, Range<usize>) {
    let replace = |character| character != '\n' && character != '\t' && unsafe_display(character);
    if !source.chars().any(replace) {
        return (Cow::Borrowed(source), span);
    }
    let mut output = String::new();
    let mut start = None;
    let mut end = None;
    for (offset, character) in source.char_indices() {
        if offset == span.start {
            start = Some(output.len());
        }
        if offset == span.end {
            end = Some(output.len());
        }
        if replace(character) {
            output.extend(character.escape_debug());
        } else {
            output.push(character);
        }
    }
    let span = start.unwrap_or(output.len())..end.unwrap_or(output.len());
    (Cow::Owned(output), span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_context_keeps_labels_on_the_original_expression() {
        let source = "let value = \"\u{1f30a}\x1b[31m\u{202e}\"; value + 1";
        let start = source.rfind("value").unwrap();
        let (display, range) = context(source, start..source.len());
        assert_eq!(&display[range], "value + 1");
        assert!(!display.chars().any(unsafe_display));
        assert!(display.contains("\\u{1b}[31m\\u{202e}"));
        let (_, eof) = context(source, source.len()..source.len());
        assert_eq!(eof, display.len()..display.len());
    }
}
