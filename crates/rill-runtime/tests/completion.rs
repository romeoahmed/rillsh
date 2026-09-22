//! Completion observes published materialized values, never evaluating user expressions.
mod support;
use rill_runtime::{Engine, Progress};
use rill_syntax::{completion::query, parse};
use support::run as finish;

fn names(engine: &Engine, source: &str) -> Vec<String> {
    engine
        .complete(&query(source, source.len()).unwrap())
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn completion_follows_publication_and_survives_collection() {
    let mut engine = Engine::standard().unwrap();
    assert!(names(&engine, "ma").contains(&"map".to_owned()));
    engine
        .begin(&parse("staged", "let unpublished = 42").unwrap())
        .unwrap();
    assert!(names(&engine, "unpub").is_empty());
    while engine.step(1).unwrap() != Progress::Complete {
        engine.collect();
    }
    assert_eq!(names(&engine, "unpub"), ["unpublished"]);
    assert!(finish(&mut engine, "let discarded = 42; 1 / 0").is_err());
    assert!(names(&engine, "discar").is_empty());
    finish(&mut engine, "let unpublished = {nested: {answer: 42}}").unwrap();
    engine.collect();
    assert_eq!(names(&engine, "unpublished.nested."), ["answer"]);
}

#[test]
fn functions_and_nominal_values_supply_metadata_without_execution() {
    let mut engine = Engine::standard().unwrap();
    finish(
        &mut engine,
        r#"
let dangerous = { () => raise (error "Called" "completion executed code") }
struct Box {value}
let boxed = Box {value: {answer: 42}}
let data = {else: 1, safe: dangerous, "not an identifier": 2, "escape\u{1b}": 3}
"#,
    )
    .unwrap();
    assert!(names(&engine, "dangerous.").is_empty());
    assert_eq!(names(&engine, "data."), ["else", "safe"]);
    assert_eq!(names(&engine, "boxed.value."), ["answer"]);
    assert_eq!(names(&engine, "Option."), ["None", "Some"]);
    assert!(names(&engine, "missing.field.").is_empty());
    assert_eq!(
        engine.complete(&query("danger", 6).unwrap()),
        [("dangerous".into(), "Function: dangerous ()".into())]
    );
}

#[test]
fn large_namespaces_are_sorted_and_bounded_without_changing_runtime_values() {
    let mut engine = Engine::default();
    let fields = (0..350)
        .rev()
        .map(|i| format!("field_{i:03}: {i}"))
        .collect::<Vec<_>>()
        .join(", ");
    finish(&mut engine, &format!("let data = {{{fields}}}")).unwrap();
    let candidates = names(&engine, "data.");
    assert_eq!(candidates.len(), 200);
    assert_eq!(candidates.first().unwrap(), "field_000");
    assert_eq!(candidates.last().unwrap(), "field_199");
    assert_eq!(
        names(&engine, "data.field_34"),
        [
            "field_340",
            "field_341",
            "field_342",
            "field_343",
            "field_344",
            "field_345",
            "field_346",
            "field_347",
            "field_348",
            "field_349"
        ]
    );
}

#[test]
fn completion_documentation_is_bounded_and_cannot_control_the_terminal() {
    let mut engine = Engine::standard().unwrap();
    let source = format!(
        "## documentation\u{1b}[2J\u{202e}{}\nfn documented value = value",
        "x".repeat(4096)
    );
    finish(&mut engine, &source).unwrap();
    let query = rill_syntax::completion::query("doc", 3).unwrap();
    let candidates = engine.complete(&query);
    assert_eq!(candidates.len(), 1);
    let (name, description) = &candidates[0];
    assert_eq!(name, "documented");
    assert!(description.contains("documentation\\u{1b}[2J\\u{202e}"));
    assert!(!description.chars().any(char::is_control));
    assert!(description.ends_with('…'));
    assert!(description.len() < 2048);
    // Escaping is presentation policy; explicit help remains ordinary String data.
    finish(&mut engine, "help documented").unwrap();
    assert!(engine.inspect(
        |value| matches!(value, rill_runtime::value::Value::String(text) if text.contains('\u{1b}') && text.len() > 4096)
    ));
}
