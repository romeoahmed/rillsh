//! Bounded human-readable tables. Presentation never evaluates or consumes a value.
use crate::value::{Value, summary};
use std::fmt::{self, Write};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const ROWS: usize = 100;
const COLUMNS: usize = 8;

/// Render materialized Lists and Records as width-limited tables, other values as summaries.
/// Unit has no output. Abbreviation is for display, not serialization.
#[must_use]
pub fn render(value: Value<'_>, width: usize) -> Option<String> {
    match value {
        Value::Record(fields) => Some(table(
            vec!["field".into(), "value".into()],
            fields
                .iter()
                .take(ROWS)
                .map(|(name, value)| vec![escaped(name), cell(*value)])
                .collect(),
            width,
            fields.len().saturating_sub(ROWS),
        )),
        Value::List(values) if !values.as_slice().is_empty() => {
            let values = values.as_slice();
            let names = match &values[0] {
                Value::Record(first) => first.keys().take(COLUMNS).collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            if !names.is_empty()
                && values.iter().take(ROWS).all(|value| {
                    matches!(value, Value::Record(fields) if names.iter().all(|name| fields.contains_key(*name)))
                })
            {
                let rows = values.iter().take(ROWS).map(|value| {
                    let Value::Record(fields) = value else {
                        unreachable!("validated record row")
                    };
                    names.iter().map(|name| cell(fields[*name])).collect()
                }).collect();
                let mut output = table(
                    names.iter().map(|name| escaped(name)).collect(),
                    rows,
                    width,
                    values.len().saturating_sub(ROWS),
                );
                if values.iter().take(ROWS).any(|value| {
                    matches!(value, Value::Record(fields) if fields.len() > names.len())
                }) {
                    output.push('\n');
                    output.push_str(&fit("… additional fields omitted", width));
                }
                Some(output)
            } else {
                Some(table(
                    vec!["#".into(), "value".into()],
                    values.iter().take(ROWS).enumerate().map(|(index, value)| {
                        vec![index.to_string(), cell(*value)]
                    }).collect(),
                    width,
                    values.len().saturating_sub(ROWS),
                ))
            }
        }
        _ => summary(value),
    }
}

fn cell(value: Value<'_>) -> String {
    match value {
        Value::String(text) => escaped(&text),
        _ => summary(value).unwrap_or_else(|| "()".into()),
    }
}

fn escaped(text: &str) -> String {
    let mut output: String = text
        .chars()
        .take(256)
        .flat_map(char::escape_debug)
        .collect();
    if text.chars().nth(256).is_some() {
        output.push('…');
    }
    output
}

fn table(headers: Vec<String>, rows: Vec<Vec<String>>, width: usize, omitted: usize) -> String {
    let width = width.min(240);
    let available = width.saturating_sub((headers.len() - 1) * 3);
    let mut widths: Vec<_> = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| row[index].width())
                .chain([header.width()])
                .max()
                .unwrap_or(1)
                .max(1)
        })
        .collect();
    while widths.iter().sum::<usize>() > available {
        let Some((_, largest)) = widths
            .iter_mut()
            .enumerate()
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        *largest -= 1;
    }
    let line = |row: Vec<String>| {
        row.into_iter()
            .zip(&widths)
            .map(|(text, width)| {
                let text = fit(&text, *width);
                format!("{text}{}", " ".repeat(width.saturating_sub(text.width())))
            })
            .collect::<Vec<_>>()
            .join(" │ ")
    };
    let mut lines = vec![
        line(headers),
        widths
            .iter()
            .map(|width| "─".repeat(*width))
            .collect::<Vec<_>>()
            .join("─┼─"),
    ];
    lines.extend(rows.into_iter().map(line));
    if omitted != 0 {
        lines.push(format!("… {omitted} more rows"));
    }
    lines
        .into_iter()
        .map(|line| fit(&line, width))
        .collect::<Vec<_>>()
        .join("\n")
}

fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut output = String::new();
    for grapheme in text.graphemes(true) {
        let next = used + grapheme.width();
        if next >= width {
            break;
        }
        output.push_str(grapheme);
        used = next;
    }
    output.push('…');
    output
}

/// Bound formatting itself, including escaping, rather than truncating an allocated result.
pub(crate) fn preview(arguments: fmt::Arguments<'_>) -> String {
    struct Buffer(String);
    impl Write for Buffer {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let available = 1024 - self.0.len();
            if text.len() <= available {
                self.0.push_str(text);
                return Ok(());
            }
            self.0
                .push_str(&text[..text.floor_char_boundary(available)]);
            Err(fmt::Error)
        }
    }
    let mut buffer = Buffer(String::new());
    if fmt::write(&mut buffer, arguments).is_err() {
        buffer.0.push('…');
    }
    buffer.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_tables_and_large_escaped_values_remain_bounded() {
        for width in 0..32 {
            let output = table(
                vec!["column".into(); 8],
                vec![vec!["value".into(); 8]],
                width,
                2,
            );
            assert!(output.lines().all(|line| line.width() <= width));
        }
        let output = preview(format_args!("{:?}", "\x1b".repeat(10_000)));
        assert!(output.len() <= 1027);
        assert!(!output.contains('\x1b'));
        assert!(output.ends_with('…'));
    }

    #[test]
    fn cells_escape_controls_and_truncate_at_grapheme_boundaries() {
        assert_eq!(escaped("a\x1b[31m\nb"), "a\\u{1b}[31m\\nb");
        assert_eq!(fit("e\u{301}clair", 3), "e\u{301}c…");
        assert_eq!(fit("\u{754c}\u{754c}", 3), "\u{754c}…");
        let output = table(
            vec!["name".into(), "value".into()],
            vec![vec!["x".repeat(80), "y".repeat(80)]],
            24,
            3,
        );
        assert!(output.lines().all(|line| line.width() <= 24));
        assert!(output.ends_with("… 3 more rows"));
    }

    proptest::proptest! {
        #[test]
        fn escaped_tables_fit_the_requested_terminal_width(
            text in "(?s).{0,300}",
            width in 0_usize..241,
        ) {
            let text = escaped(&text);
            let output = table(
                vec!["field".into(), "value".into()],
                vec![vec![text.clone(), text]],
                width,
                1,
            );
            proptest::prop_assert!(output.lines().all(|line| line.width() <= width));
            proptest::prop_assert!(!output.chars().any(|c| c.is_control() && c != '\n'));
        }
    }
}
