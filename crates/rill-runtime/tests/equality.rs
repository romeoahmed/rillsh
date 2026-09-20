//! Equality observes values, not graph sharing, and remains cancellable under collection.
mod support;
use rill_runtime::{Engine, Progress, value::Value};
use rill_syntax::parse;
use support::run as finish;

#[test]
fn sharing_does_not_change_equality_or_require_tree_expansion() {
    let mut engine = Engine::default();
    finish(
        &mut engine,
        r"
fn graph depth leaf =
  if depth == 0 then leaf else do {
    let shared = graph (depth - 1) leaf
    [shared, shared]
  }
let a = graph 80 {x: 1, y: [2, 3]}
let b = graph 79 {y: [2, 3], x: 1}
let c = graph 80 {x: 1, y: [2, 4]}
[a == [b, b], a != c, a == a]
",
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::List(list)
        if list.as_slice().iter().all(|value| matches!(value, Value::Bool(true))))));
}

#[test]
fn validation_cannot_be_skipped_by_identity_mismatch_or_record_order() {
    let mut engine = Engine::default();
    for source in [
        "let bad = {ok: 1, hidden: { x => x }}; bad == bad",
        "[0, { x => x }] != [1, 2]",
        "[] == {hidden: { x => x }}",
        "{different: 1} == {ok: 2, hidden: { x => x }}",
        "enum Box {Wrap {value}}; Box.Wrap {value: { x => x }} == null",
    ] {
        assert_eq!(
            finish(&mut engine, source).unwrap_err().kind,
            "TypeError",
            "{source}"
        );
    }
    finish(
        &mut engine,
        r"
let [_, ..suffix] = [{ x => x }, 1, 2]
suffix == [1, 2]
",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
}

#[test]
fn nominal_identity_numeric_kind_and_native_bytes_remain_distinct() {
    let mut engine = Engine::standard().unwrap();
    finish(
        &mut engine,
        r#"
enum A {Case {value}}
enum B {Case {value}}
[
  A.Case {value: [1, 2]} == A.Case {value: [1, 2]},
  A.Case {value: 1} != B.Case {value: 1},
  1 != 1.0,
  -0.0 == 0.0,
  {a: 1, b: 2} != {a: 1, c: 2},
  bytes [255, 0] == bytes [255, 0],
  path (bytes [255, 47, 97]) == path (bytes [255, 47, 97]),
  path "a//b" != path "a/b"
]
"#,
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::List(list)
        if list.as_slice().iter().all(|value| matches!(value, Value::Bool(true))))));
}

#[test]
fn comparisons_yield_retain_roots_and_cancel_without_publishing() {
    let mut engine = Engine::default();
    let values = (0..4096)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    finish(
        &mut engine,
        &format!("let left = [{values}]; let right = [{values}]"),
    )
    .unwrap();
    engine
        .begin(&parse("cancelled-comparison", "let pending = left == right").unwrap())
        .unwrap();
    assert_eq!(engine.step(32).unwrap(), Progress::Yielded);
    engine.collect();
    assert!(engine.interrupt().unwrap_err().is_cancelled());
    engine.collect();
    assert_eq!(
        finish(&mut engine, "pending").unwrap_err().kind,
        "NameError"
    );
    finish(&mut engine, "left == right").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
}

#[test]
fn long_byte_values_compare_across_quanta_and_keep_the_original_error_span() {
    let mut engine = Engine::default();
    let text = "x".repeat(128 * 1024);
    finish(
        &mut engine,
        &format!("let make = {{ () => {text:?} + \"!\" }}; let a = make (); let b = make ()"),
    )
    .unwrap();
    engine
        .begin(&parse("byte-comparison", "a == b").unwrap())
        .unwrap();
    assert_eq!(engine.step(8).unwrap(), Progress::Yielded);
    while engine.step(8).unwrap() != Progress::Complete {
        engine.collect();
    }
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    finish(&mut engine, "fn invalid () = [0, { x => x }] == [1, 2]").unwrap();
    let error = finish(&mut engine, "invalid ()").unwrap_err();
    assert_eq!(error.kind, "TypeError");
    let origin = error.origin.unwrap();
    assert!(origin.text.starts_with("fn invalid"));
    assert!(origin.text[error.span.unwrap()].contains("=="));
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn graph_equality_matches_unshared_lists(
        values in proptest::collection::vec(-100_i64..100, 0..30),
        depth in 0_usize..8,
        changed in proptest::prelude::any::<bool>(),
    ) {
        let left = format!("{values:?}");
        let mut right = values;
        if changed { right.push(101); }
        let right = format!("{right:?}");
        let source = format!(r"
fn graph depth leaf = if depth == 0 then leaf else do {{
  let value = graph (depth - 1) leaf
  [value, value]
}}
graph {depth} {left} == graph {depth} {right}
");
        let mut engine = Engine::default();
        finish(&mut engine, &source).unwrap();
        proptest::prop_assert!(engine.inspect(|value| matches!(value, Value::Bool(equal) if equal != changed)));
    }
}
