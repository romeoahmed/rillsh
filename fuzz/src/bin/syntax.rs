//! Decode, parse, validate and lower source without executing it or importing files.
fn main() {
    afl::fuzz!(|data: &[u8]| {
        if data.len() > 8192 {
            return;
        }
        let Ok(source) = std::str::from_utf8(data) else {
            return;
        };
        let mut end = 0;
        for token in rill_syntax::token::highlight_tokens(source) {
            assert!(end <= token.span.start && token.span.start < token.span.end);
            assert!(source.get(token.span.clone()).is_some());
            end = token.span.end;
        }
        let literal = rill_syntax::completion::path_source(data, false);
        let tokens = rill_syntax::token::lex(&literal).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(&tokens[0].kind, rill_syntax::token::Kind::String(text) if text == source)
        );
        for cursor in [0, source.len() / 2, source.len()] {
            if let Some(query) = rill_syntax::completion::query(source, cursor) {
                let _ = query.local_names(source);
                assert!(query.span.start <= cursor && cursor <= query.span.end);
                assert!(source.is_char_boundary(query.span.start));
                assert!(source.is_char_boundary(query.span.end));
                assert_eq!(&source[query.span.start..cursor], query.prefix);
            }
            if let Some(query) = rill_syntax::completion::path_query(source, cursor) {
                assert!(query.span.start <= cursor && cursor <= query.span.end);
                assert!(source.get(query.span).is_some());
            }
        }
        match rill_syntax::parse("fuzz", source) {
            Ok(module) => {
                let formatted = rill_syntax::format::format(source).unwrap();
                assert_eq!(rill_syntax::format::format(&formatted).unwrap(), formatted);
                let kinds = |text: &str| {
                    let mut kinds: Vec<_> = rill_syntax::token::lex(text)
                        .unwrap()
                        .into_iter()
                        .map(|token| token.kind)
                        .collect();
                    // Formatting may finish the final line, but interior newlines
                    // separate statements and must not disappear or multiply.
                    if !text.is_empty() && !text.ends_with('\n') {
                        kinds.push(rill_syntax::token::Kind::Newline);
                    }
                    kinds
                };
                assert_eq!(kinds(source), kinds(&formatted));
                let mut engine = rill_runtime::Engine::default();
                // Beginning lowers code; imported files are opened only after stepping.
                engine.begin(&module).unwrap();
            }
            Err(errors) => {
                for error in errors {
                    assert!(error.span.start <= error.span.end);
                    assert!(source.is_char_boundary(error.span.start));
                    assert!(source.is_char_boundary(error.span.end));
                }
            }
        }
    });
}
