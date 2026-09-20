//! Dedicated source fixtures for grammar, completeness and whole-entry validation.
use rill_syntax::{
    ast::{Binary, ExprKind, Statement},
    parse,
};

#[test]
fn parses_standard_library() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rill-runtime/stdlib");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let source = std::fs::read_to_string(&path).unwrap();
        if let Err(errors) = parse("stdlib", &source) {
            panic!("{}: {errors:?}", path.display());
        }
    }
}

#[test]
fn application_precedes_arithmetic() {
    let module = parse("test", "f x + g y").unwrap();
    let Statement::Expression(id) = module.statements[0] else {
        panic!("expected expression")
    };
    let ExprKind::Binary(Binary::Add, left, right) = module.expression(id).kind else {
        panic!("expected addition")
    };
    assert!(matches!(module.expression(left).kind, ExprKind::Call(..)));
    assert!(matches!(module.expression(right).kind, ExprKind::Call(..)));
}

#[test]
fn complete_forms() {
    for source in [
        "()",
        "{}",
        "f",
        "f x y",
        "f [0]",
        "items[0]",
        "f (-1)",
        "{ x => x }",
        "{ () => }",
        "rec { loop n => if n == 0 then 0 else loop (n - 1) }",
        "let {a, ..rest} = {a: 1, b: 2}",
        "fn f a b = a + b",
        "match x of { [head, ..tail] => head, _ => 0 }",
        "job { ^printf '%s' --flag /tmp a-b if | ^cat }",
        "^printf '%s' $(f (g x)) ...$args",
        "^echo foo#bar # comment",
        "do { let f = { a b => a + b }; f 1 2 }",
    ] {
        assert!(
            parse("test", source).is_ok(),
            "{source}: {:?}",
            parse("test", source).err()
        );
    }
}

#[test]
fn rejects_removed_or_ambiguous_call_forms() {
    for source in ["f(x)", "f(1, 2)", "{ => 1 }", "_"] {
        assert!(parse("test", source).is_err(), "accepted {source}");
    }
}

#[test]
fn whole_entry_validation_includes_unreachable_syntax() {
    for source in [
        "if false then { [x, x] => x } else 1",
        "let x = 1; let x = 2",
        "do { enum Local {A} }",
        "{a: 1, a: 2}",
        "export {x}; export {y}",
        "struct _ {}",
    ] {
        assert!(parse("test", source).is_err(), "accepted {source}");
    }
}

#[test]
fn numbers_are_checked_before_any_effect() {
    for source in ["9223372036854775808", "-9223372036854775809", "1e309"] {
        assert!(parse("test", source).is_err(), "accepted {source}");
    }
    for source in [
        "9223372036854775807",
        "-9223372036854775808",
        "1_000",
        "1.25e-10",
    ] {
        assert!(parse("test", source).is_ok(), "rejected {source}");
    }
}

#[test]
fn command_words_remain_distinct_and_redirections_are_ordered() {
    for source in [
        "^echo 'a'b",
        "^echo $x/suffix",
        "^...$items",
        "^cat > ...$paths",
        "^echo 3>file",
        "^cat |> f",
    ] {
        assert!(parse("test", source).is_err(), "accepted {source}");
    }
    for source in [
        "^echo 2>&1 hello",
        "^echo 2>&1 >file",
        "^echo >file 2>&1",
        "^echo --flag /tmp a-b if foo#bar",
        "^echo $(job { ^cat })",
    ] {
        assert!(
            parse("test", source).is_ok(),
            "rejected {source}: {:?}",
            parse("test", source).err()
        );
    }
}

#[test]
fn delimiters_change_newline_context_but_blocks_restore_it() {
    for source in [
        "(f\n x)",
        "(f \n x)",
        "fn f# header\n x = x",
        "{x \n y => x}",
        "[f\n x]",
        "{a: f\n x}",
        "[do {f\nx}]",
        "[ { x => x\nx } ]",
        "items[\nf\nx\n]",
        "1\n|> f",
        "{with: 1}.with",
    ] {
        assert!(
            parse("test", source).is_ok(),
            "rejected {source}: {:?}",
            parse("test", source).err()
        );
    }
    let module = parse("test", "f\nx").unwrap();
    assert_eq!(module.statements.len(), 2);
    for source in ["1 < 2 < 3", "{'name'}", "{of}"] {
        assert!(parse("test", source).is_err(), "accepted {source}");
    }
    assert!(parse("test", "(1 < 2) == true").is_ok());
}

#[test]
fn incomplete_entries_can_be_extended() {
    for source in [
        "{x",
        "{x y",
        "[1,",
        "^cat |",
        "job { ^cat",
        "rec { loop n =>",
        "if true then 1 else",
        "\"unterminated",
    ] {
        let errors = parse("test", source).unwrap_err();
        assert!(errors.iter().all(|e| e.incomplete), "{source}: {errors:?}");
    }
}

#[test]
fn syntax_depth_is_a_language_limit() {
    for source in [
        format!("{}true", "not ".repeat(300)),
        format!("{}0{}", "[".repeat(300), "]".repeat(300)),
    ] {
        let errors = parse("test", &source).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("nesting")));
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn arbitrary_source_has_valid_diagnostic_boundaries(source in "(?s).{0,200}") {
        if let Err(errors) = parse("property", &source) {
            for error in errors {
                proptest::prop_assert!(error.span.start <= error.span.end);
                proptest::prop_assert!(error.span.end <= source.len());
                proptest::prop_assert!(source.is_char_boundary(error.span.start));
                proptest::prop_assert!(source.is_char_boundary(error.span.end));
            }
        }
    }
}

#[test]
fn invalid_escape_diagnostics_include_the_complete_unicode_scalar() {
    let source = "\"\\\u{a1}";
    let error = rill_syntax::token::lex(source).unwrap_err();
    assert!(!error.incomplete);
    assert_eq!(error.span, 0..source.len());
    for source in ["\"\\u{+41}\"", "\"\\u{X", "\"\\u{d800}"] {
        assert!(!rill_syntax::token::lex(source).unwrap_err().incomplete);
    }
}
