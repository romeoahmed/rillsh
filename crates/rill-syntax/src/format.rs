//! Conservative source formatting preserves tokens, comments and literal contents.
use crate::{
    Diagnostic, parse,
    token::{Kind, highlight_tokens},
};

/// Normalize structural indentation and trailing horizontal whitespace.
/// Multiline strings are copied verbatim, including their leading whitespace.
///
/// # Errors
/// Returns syntax diagnostics without producing a partial formatted program.
pub fn format(source: &str) -> Result<String, Vec<Diagnostic>> {
    parse("<format>", source)?;
    let tokens = highlight_tokens(source);
    let mut result = String::with_capacity(source.len());
    let mut depth = 0_usize;
    let mut offset = 0;
    let mut token_index = 0;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        let first = token_index;
        while token_index < tokens.len() && tokens[token_index].span.start < end {
            token_index += 1;
        }
        let current = &tokens[first..token_index];
        let literal = tokens[..first]
            .last()
            .is_some_and(|token| matches!(token.kind, Kind::String(_)) && token.span.end > offset)
            || current
                .iter()
                .any(|token| matches!(token.kind, Kind::String(_)) && token.span.end > end);
        if literal {
            result.push_str(line);
        } else {
            let text = line.trim_matches([' ', '\t', '\r', '\n']);
            let closing = current
                .iter()
                .take_while(|token| {
                    matches!(
                        token.kind,
                        Kind::CloseBrace | Kind::CloseList | Kind::CloseParen
                    )
                })
                .count();
            if !text.is_empty() {
                result.push_str(&"  ".repeat(depth - closing));
                result.push_str(text);
            }
            result.push('\n');
        }
        for token in current {
            match token.kind {
                Kind::OpenBrace | Kind::OpenList | Kind::OpenParen => depth += 1,
                Kind::CloseBrace | Kind::CloseList | Kind::CloseParen => {
                    depth -= 1;
                }
                _ => {}
            }
        }
        offset = end;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::lex;
    #[test]
    fn formatting_keeps_comments_and_statement_boundaries() {
        let source = "do {\n\t# explain\n let x = [\n 1,\n2\n ]\nx\n}\n# final";
        assert_eq!(
            format(source).unwrap(),
            "do {\n  # explain\n  let x = [\n    1,\n    2\n  ]\n  x\n}\n# final\n"
        );
    }
    #[test]
    fn formatting_is_idempotent_and_preserves_literals_comments_and_tokens() {
        for source in [
            "do {\nlet x = [\n1,\n2\n]\nx\n}\n",
            "# explanation\n[1, 2]\n  |> map { x =>\n  x + 1\n}  \n",
            "let value = \"first\n   second  \nthird\"\nvalue",
            "^printf '%s' 'two words'\n",
        ] {
            let formatted = format(source).unwrap();
            assert_eq!(format(&formatted).unwrap(), formatted);
            let kinds = |source: &str| {
                lex(source)
                    .unwrap()
                    .into_iter()
                    .filter(|token| token.kind != Kind::Newline)
                    .map(|token| token.kind)
                    .collect::<Vec<_>>()
            };
            assert_eq!(kinds(source), kinds(&formatted));
            parse("formatted", &formatted).unwrap();
        }
    }
}
