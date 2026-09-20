//! Decode, parse, validate and lower source without executing it or importing files.
fn main() {
    afl::fuzz!(|data: &[u8]| {
        if data.len() > 8192 {
            return;
        }
        let Ok(source) = std::str::from_utf8(data) else {
            return;
        };
        let literal = rill_syntax::completion::path_source(data, false);
        let tokens = rill_syntax::token::lex(&literal).unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(
            matches!(&tokens[0].kind, rill_syntax::token::Kind::String(text) if text == source)
        );
        for cursor in [0, source.len() / 2, source.len()] {
            if let Some(query) = rill_syntax::completion::query(source, cursor) {
                assert!(query.span.start <= cursor && cursor <= query.span.end);
                assert!(source.is_char_boundary(query.span.start));
                assert!(source.is_char_boundary(query.span.end));
                assert_eq!(&source[query.span.start..cursor], query.prefix);
            }
        }
        match rill_syntax::parse("fuzz", source) {
            Ok(module) => {
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
