//! Completion context uses the execution lexer and never evaluates an expression.
use crate::token::{Kind, Span, lex};

/// An identifier prefix or a chain of materialized field projections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub parents: Vec<String>,
    pub prefix: String,
    pub span: Span,
}

/// An explicit command word or quoted path prefix; no filesystem access occurs here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathQuery {
    pub prefix: String,
    pub span: Span,
    pub command: bool,
    pub executable: bool,
}

/// Find a path completion context using the execution lexer's quote and command modes.
#[must_use]
pub fn path_query(source: &str, cursor: usize) -> Option<PathQuery> {
    const MARKER: &str = "rill_path_completion_marker";
    if source.len() > 1024 * 1024 {
        return None;
    }
    let before = source.get(..cursor)?;
    for close in ["", "\"", "'"] {
        let probe = format!("{before}{MARKER}{close}");
        let Ok(mut tokens) = lex(&probe) else {
            continue;
        };
        let Some(target) = tokens.pop() else { continue };
        let prefix = match &target.kind {
            Kind::Word(word) | Kind::String(word) => word.strip_suffix(MARKER),
            _ => None,
        };
        let Some(prefix) = prefix else { continue };
        if target.span.end != probe.len() {
            continue;
        }
        let start = target.span.start;
        let command = lex(&format!("{}{MARKER}", source.get(..start)?))
            .ok()?
            .last()
            .is_some_and(|token| matches!(token.kind, Kind::Word(_)));
        let executable = command
            && tokens
                .last()
                .is_some_and(|token| token.kind == Kind::Command);
        let end = lex(source)
            .ok()
            .and_then(|tokens| {
                tokens
                    .into_iter()
                    .find(|token| token.span.start == start)
                    .map(|token| token.span.end)
            })
            .or_else(|| (cursor == source.len()).then_some(cursor))?;
        return Some(PathQuery {
            prefix: prefix.into(),
            span: start..end,
            command,
            executable,
        });
    }
    None
}

/// Produce source that denotes one native path, never executable filename contents.
#[must_use]
pub fn path_source(bytes: &[u8], command: bool) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        let mut source = String::from("\"");
        for character in text.chars() {
            // Rust character escapes include \' and \0; Rill strings use neither.
            match character {
                '\'' => source.push(character),
                '\0' => source.push_str("\\u{0}"),
                _ => source.extend(character.escape_debug()),
            }
        }
        source.push('"');
        return source;
    }
    let bytes = bytes
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}(path (bytes [{bytes}]))", if command { "$" } else { "" })
}

/// Find a replaceable identifier at a UTF-8 cursor boundary.
/// Commands, comments, strings and computed receivers are deliberately excluded.
#[must_use]
pub fn query(source: &str, cursor: usize) -> Option<Query> {
    const MARKER: &str = "rill_completion_marker";
    if source.len() > 1024 * 1024 {
        return None;
    }
    let before = source.get(..cursor)?;
    // The marker asks the contextual lexer which language mode owns the cursor,
    // including empty prefixes, command substitutions and unfinished expressions.
    let probe = format!("{before}{MARKER}");
    let mut tokens = lex(&probe).ok()?;
    let target = tokens.pop()?;
    if !matches!(target.kind, Kind::Name(_)) || target.span.end != probe.len() {
        return None;
    }
    let start = target.span.start;
    let prefix = before.get(start..)?.to_owned();
    let end = cursor
        + source
            .get(cursor..)?
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
    let mut parents = Vec::new();
    while tokens.last().is_some_and(|token| token.kind == Kind::Dot) {
        let dot = tokens.pop()?;
        if dot.separated {
            return None;
        }
        let base = tokens.pop()?;
        parents.push(base.kind.field_name()?.to_owned());
    }
    parents.reverse();
    if parents.is_empty()
        && !target.separated
        && tokens.last().is_some_and(|token| {
            matches!(
                token.kind,
                Kind::Number(_)
                    | Kind::String(_)
                    | Kind::Word(_)
                    | Kind::CloseParen
                    | Kind::CloseList
                    | Kind::CloseBrace
            )
        })
    {
        return None;
    }
    Some(Query {
        parents,
        prefix,
        span: start..end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::strategy::Strategy;

    #[test]
    fn path_literals_preserve_quotes_and_control_characters() {
        for text in ["'", "\"", "\\0", "\0", "a'\"\\\n\r\t\0\u{202e}"] {
            let literal = path_source(text.as_bytes(), false);
            let tokens = lex(&literal).unwrap();
            assert_eq!(tokens.len(), 1);
            assert_eq!(tokens[0].kind, Kind::String(text.into()));
        }
    }

    #[test]
    fn filesystem_contexts_use_quoted_values_and_replace_the_whole_token() {
        for (source, prefix, command, executable) in [
            ("^pri", "pri", true, true),
            ("^cat file", "file", true, false),
            ("^cat \"space na", "space na", true, false),
            ("read_text \"space na", "space na", false, false),
            ("^cat ", "", true, false),
        ] {
            let query = path_query(source, source.len()).unwrap();
            assert_eq!(
                (&*query.prefix, query.command, query.executable),
                (prefix, command, executable),
                "{source}"
            );
        }
        let query = path_query("^cat filename", 7).unwrap();
        assert_eq!(query.prefix, "fi");
        assert_eq!(query.span, 5..13);
        for source in ["# comment", "^cat # comment", "map", "value.field"] {
            assert!(path_query(source, source.len()).is_none(), "{source}");
        }
    }

    #[test]
    fn context_comes_from_language_modes_and_preserves_replacement_boundaries() {
        for (source, parents, prefix) in [
            ("ma", vec![], "ma"),
            ("map ", vec![], ""),
            ("Option.So", vec!["Option"], "So"),
            ("module.record.", vec!["module", "record"], ""),
            ("^printf $va", vec![], "va"),
            ("^printf $(value.fi", vec!["value"], "fi"),
            ("do {\n  list |> ma", vec![], "ma"),
            ("data.else", vec!["data"], "else"),
        ] {
            let query = query(source, source.len()).unwrap();
            assert_eq!(query.parents, parents, "{source}");
            assert_eq!(query.prefix, prefix, "{source}");
            assert_eq!(&source[query.span], prefix, "{source}");
        }
        for source in [
            "^ec",
            "^echo ma",
            "# ma",
            "'ma",
            "\"ma",
            "call ().fi",
            "list[0].fi",
            "data .fi",
            "12ma",
            "'str'ma",
        ] {
            assert!(query(source, source.len()).is_none(), "{source}");
        }
        let source = "'\u{754c}'; mapping + 2";
        let cursor = source.find("mapping").unwrap() + 2;
        let query = query(source, cursor).unwrap();
        assert_eq!(query.prefix, "ma");
        assert_eq!(&source[query.span], "mapping");
        assert!(super::query(source, 2).is_none());
        assert!(super::query(source, source.len() + 1).is_none());
    }
    proptest::proptest! {
        #[test]
        fn generated_path_literals_round_trip_arbitrary_unicode(
            text in proptest::collection::vec(proptest::char::any(), 0..128)
                .prop_map(|characters| characters.into_iter().collect::<String>()),
        ) {
            let literal = path_source(text.as_bytes(), false);
            let tokens = lex(&literal).unwrap();
            proptest::prop_assert_eq!(tokens.len(), 1);
            proptest::prop_assert_eq!(&tokens[0].kind, &Kind::String(text));
            proptest::prop_assert!(crate::parse("completion", &literal).is_ok());
            let command = format!("^echo {literal}");
            proptest::prop_assert!(crate::parse("completion", &command).is_ok());
        }

        #[test]
        fn arbitrary_source_and_cursor_never_split_a_unicode_scalar(
            chars in proptest::collection::vec(proptest::char::any(), 0..256),
            offset in proptest::prelude::any::<usize>(),
        ) {
            let source: String = chars.into_iter().collect();
            let cursor = offset % (source.len() + 1);
            if let Some(query) = query(&source, cursor) {
                proptest::prop_assert!(query.span.start <= cursor && cursor <= query.span.end);
                proptest::prop_assert!(source.is_char_boundary(query.span.start));
                proptest::prop_assert!(source.is_char_boundary(query.span.end));
                proptest::prop_assert_eq!(&source[query.span.start..cursor], query.prefix);
            }
        }
    }
}
