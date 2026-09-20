//! Shared valid sources, semantic whitespace, query compilation and incremental tree parity.
use tree_sitter::{InputEdit, Parser, Point, Query, Tree};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rill::LANGUAGE.into())
        .unwrap();
    parser
}
fn valid(parser: &mut Parser, source: &str) -> Tree {
    assert!(
        rill_syntax::parse("fixture", source).is_ok(),
        "invalid fixture: {source}"
    );
    let tree = parser.parse(source, None).unwrap();
    assert!(
        !tree.root_node().has_error(),
        "{source}\n{}",
        tree.root_node().to_sexp()
    );
    tree
}
#[test]
fn bundled_modules_have_no_error_nodes() {
    let mut parser = parser();
    let library = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rill-runtime/stdlib");
    for entry in std::fs::read_dir(library).unwrap() {
        valid(
            &mut parser,
            &std::fs::read_to_string(entry.unwrap().path()).unwrap(),
        );
    }
}
#[test]
fn application_and_newlines_keep_their_execution_meaning() {
    let mut parser = parser();
    for source in [
        "(f\n x)",
        "[f\n x]",
        "{a: f\n x}",
        "[do {f\nx}]",
        "[{ x => x\nx }]",
        "items[\nf\nx\n]",
        "1\n|> f",
        "{with: 1}.with",
        "fn f# header\n x = x",
        "{x\n y => x}",
        "fn f -1 = 0",
        "fn f Option.Some {value} = value",
    ] {
        valid(&mut parser, source);
    }
    let tree = valid(&mut parser, "f\nx");
    assert_eq!(tree.root_node().named_child_count(), 2);
    let tree = valid(&mut parser, "f x y");
    let call = tree.root_node().named_child(0).unwrap();
    assert_eq!(call.kind(), "call_expression");
    assert_eq!(
        call.child_by_field_name("function").unwrap().kind(),
        "call_expression"
    );
    for source in [
        "f(x)",
        "fn f(x) = x",
        "{x(y) => x}",
        "^echo\"x\"",
        "x [0].",
        "fn f x =",
        "job { ^cat |",
    ] {
        assert!(
            parser.parse(source, None).unwrap().root_node().has_error(),
            "accepted {source}"
        );
    }
}

#[test]
fn bare_command_arguments_keep_expression_punctuation_as_data() {
    let mut parser = parser();
    for source in [
        "^printf %s (value)",
        "^printf %s [value]",
        "^printf %s a(b)[c]",
        "^printf %s a\u{000b}b",
        "^#executable argument#suffix",
        "job { ^printf %s [value] | ^cat }",
    ] {
        valid(&mut parser, source);
    }
}
fn shape(tree: &Tree) -> Vec<(String, std::ops::Range<usize>, bool)> {
    let mut result = Vec::new();
    let mut nodes = vec![tree.root_node()];
    while let Some(node) = nodes.pop() {
        result.push((node.kind().to_owned(), node.byte_range(), node.is_missing()));
        for index in (0..node.child_count()).rev() {
            nodes.push(node.child(index).unwrap());
        }
    }
    result
}
#[test]
fn incremental_edits_match_fresh_parse_including_ranges() {
    let mut parser = parser();
    for (before, start, old_end, replacement) in [
        ("f x", 1, 2, "\n"),
        ("(f x)", 2, 3, "\n"),
        ("job { ^printf hi }", 14, 16, "$name"),
        ("{x => x}", 6, 7, "x\n x"),
        ("let value = [1, 2]", 16, 17, ""),
    ] {
        let mut previous = parser.parse(before, None).unwrap();
        let mut after = before.to_owned();
        after.replace_range(start..old_end, replacement);
        let mut new_end = Point::new(0, start);
        for byte in replacement.bytes() {
            if byte == b'\n' {
                new_end.row += 1;
                new_end.column = 0;
            } else {
                new_end.column += 1;
            }
        }
        previous.edit(&InputEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: start + replacement.len(),
            start_position: Point::new(0, start),
            old_end_position: Point::new(0, old_end),
            new_end_position: new_end,
        });
        let incremental = parser.parse(&after, Some(&previous)).unwrap();
        let fresh = parser.parse(&after, None).unwrap();
        assert_eq!(shape(&incremental), shape(&fresh), "{after}");
    }
}
#[test]
fn queries_compile_against_the_generated_language() {
    let language = tree_sitter_rill::LANGUAGE.into();
    for source in [
        tree_sitter_rill::HIGHLIGHTS_QUERY,
        tree_sitter_rill::LOCALS_QUERY,
    ] {
        Query::new(&language, source).unwrap();
    }
}

#[test]
fn local_queries_identify_recursive_bindings_parameters_and_nested_scopes() {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let source = "let count = rec { loop n => if n == 0 then 0 else loop (n - 1) }; fn apply f value = f value";
    let tree = valid(&mut parser(), source);
    let query = Query::new(
        &tree_sitter_rill::LANGUAGE.into(),
        tree_sitter_rill::LOCALS_QUERY,
    )
    .unwrap();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), source.as_bytes());
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    let mut scopes = Vec::new();
    while let Some((matched, index)) = captures.next() {
        let capture = matched.captures()[*index];
        let text = &source[capture.node.byte_range()];
        match query.capture_names()[usize::try_from(capture.index).unwrap()] {
            "local.definition" => definitions.push(text),
            "local.reference" => references.push(text),
            "local.scope" => scopes.push(capture.node.kind()),
            name => panic!("unexpected local capture {name}"),
        }
    }
    for name in ["count", "loop", "n", "apply", "f", "value"] {
        assert!(definitions.contains(&name), "missing binding {name}");
    }
    assert!(references.contains(&"loop"));
    assert!(scopes.contains(&"recursive_closure"));
    assert!(scopes.contains(&"function_body"));
}

#[test]
fn local_queries_distinguish_destructuring_bindings_from_field_names() {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let source = "let outside = 1; fn use [x, ..tail] {key: y, short, ..rest} = do { let nested = x; { param => outside + y + short + param } }; use";
    let tree = valid(&mut parser(), source);
    let query = Query::new(
        &tree_sitter_rill::LANGUAGE.into(),
        tree_sitter_rill::LOCALS_QUERY,
    )
    .unwrap();
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(&query, tree.root_node(), source.as_bytes());
    let mut definitions = Vec::new();
    let mut references = Vec::new();
    let mut function_scope = None;
    while let Some((matched, index)) = captures.next() {
        let capture = matched.captures()[*index];
        let text = &source[capture.node.byte_range()];
        match query.capture_names()[usize::try_from(capture.index).unwrap()] {
            "local.definition" => definitions.push((text, capture.node.start_byte())),
            "local.reference" => references.push(text),
            "local.scope" if capture.node.kind() == "function_body" => {
                function_scope = Some(capture.node.byte_range());
            }
            "local.scope" => {}
            name => panic!("unexpected capture {name}"),
        }
    }
    assert_eq!(
        definitions
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        [
            "outside", "use", "x", "tail", "y", "short", "rest", "nested", "param"
        ]
    );
    assert_eq!(references, ["x", "outside", "y", "short", "param", "use"]);
    let function_scope = function_scope.unwrap();
    assert!(
        !function_scope.contains(&definitions[1].1),
        "function name belongs to the outer scope"
    );
    assert!(
        function_scope.contains(&definitions[2].1),
        "parameters belong to the function scope"
    );
}
