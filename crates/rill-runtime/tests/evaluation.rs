//! Language values, lexical captures and publication across VM quanta and collection.
use rill_runtime::{Engine, Progress, value::Value};
use rill_syntax::parse;

mod support;

fn execute(engine: &mut Engine, source: &str) {
    support::run(engine, source).unwrap();
}

fn integer(engine: &Engine) -> i64 {
    engine.inspect(|value| {
        let Value::Int(value) = value else {
            panic!("expected Int, got {}", value.kind());
        };
        value
    })
}

#[test]
fn arithmetic_and_ordered_application() {
    let mut engine = Engine::default();
    execute(&mut engine, "fn add a b = a + b; 20 |> add 22");
    assert_eq!(integer(&engine), 42);
    execute(
        &mut engine,
        "let x = 10; let f = { y => x + y }; do { let x = 99; f 5 }",
    );
    assert_eq!(integer(&engine), 15);
}

#[test]
fn recursive_root_survives_partial_application_and_collection() {
    let mut engine = Engine::default();
    engine.begin(&parse("test", "let count = rec { loop total n => if n == 0 then total else loop (total + 1) (n - 1) }; count 0 100000").unwrap()).unwrap();
    while engine.step(100).unwrap() != Progress::Complete {
        engine.collect();
    }
    assert_eq!(integer(&engine), 100_000);
}

#[test]
fn failed_entries_do_not_publish_bindings() {
    let mut engine = Engine::default();
    execute(&mut engine, "let x = 5");
    engine
        .begin(&parse("test", "let x = 9; 9223372036854775807 + 1").unwrap())
        .unwrap();
    assert_eq!(
        support::finish(&mut engine).unwrap_err().kind,
        "ArithmeticError"
    );
    execute(&mut engine, "x");
    assert_eq!(integer(&engine), 5);
}

#[test]
fn closures_retain_lexical_values_from_finished_scopes() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        "let f = do { let unused = { () => 0 }; let x = 42; { () => x } }; f ()",
    );
    assert_eq!(integer(&engine), 42);
}

#[test]
fn list_patterns_are_atomic_and_keep_suffixes_alive() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        "let [head, ..tail] = [1, 2, 3]; let f = { [x, y] => x + y }; f tail",
    );
    assert_eq!(integer(&engine), 5);
}

#[test]
fn structural_equality_rejects_hidden_functions() {
    let mut engine = Engine::default();
    engine
        .begin(&parse("test", "[0, { x => x }] == [1, 2]").unwrap())
        .unwrap();
    assert_eq!(support::finish(&mut engine).unwrap_err().kind, "TypeError");
    execute(&mut engine, "{a: 1, b: 2} == {b: 2, a: 1}");
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
}

#[test]
fn short_circuit_does_not_evaluate_the_other_operand() {
    let mut engine = Engine::default();
    execute(&mut engine, "false and missing");
    assert!(engine.inspect(|v| matches!(v, Value::Bool(false))));
    execute(&mut engine, "true or missing");
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
}

#[test]
fn matching_uses_lexical_nominal_identity_and_guards() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        "enum Option {None, Some {value}}; let f = { (Option.Some {value}) => value + 1 }; match Option.Some {value: 41} of { Option.Some {value} if value < 0 => 0, v => f v }",
    );
    assert_eq!(integer(&engine), 42);
    execute(
        &mut engine,
        "struct A {x}; struct B {x}; A {x: 1} == B {x: 1}",
    );
    assert!(engine.inspect(|v| matches!(v, Value::Bool(false))));
}

#[test]
fn mutual_tail_calls_return_the_expected_value() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        "rec { fn even n = if n == 0 then true else odd (n - 1); fn odd n = if n == 0 then false else even (n - 1) }; even 10000",
    );
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
}

#[test]
fn constructors_validate_key_sets_and_store_declaration_order() {
    let mut engine = Engine::default();
    execute(&mut engine, "struct Point {x, y}; Point {y: 2, x: 1}");
    engine.inspect(|value| {
        let Value::Adt(value) = value else {
            panic!("nominal Point");
        };
        assert_eq!(
            value.fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["x", "y"]
        );
        assert!(matches!(value.fields["x"], Value::Int(1)));
        assert!(matches!(value.fields["y"], Value::Int(2)));
    });
    for source in [
        "Point {x: 1}",
        "Point {x: 1, z: 2}",
        "Point {x: 1, y: 2, z: 3}",
    ] {
        engine.begin(&parse("shape", source).unwrap()).unwrap();
        assert_eq!(support::finish(&mut engine).unwrap_err().kind, "TypeError");
    }
}

#[test]
fn pattern_mismatch_retries_without_publishing_partial_bindings() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        "let x = 7; match [1, 2] of { [x, 3] => 0, _ => x }",
    );
    assert_eq!(integer(&engine), 7);
}

#[test]
fn parameter_failure_precedes_evaluation_of_later_arguments() {
    let mut engine = Engine::default();
    engine
        .begin(&parse("test", "let f = { 0 x => x }; f 1 missing").unwrap())
        .unwrap();
    assert_eq!(support::finish(&mut engine).unwrap_err().kind, "MatchError");
}

#[test]
fn signed_boundary_and_immutable_updates() {
    let mut engine = Engine::default();
    execute(&mut engine, "-9223372036854775808");
    assert_eq!(integer(&engine), i64::MIN);
    execute(
        &mut engine,
        "let base = {x: 1, y: 2}; let updated = base with {x: 40}; base.x + updated.x + 1",
    );
    assert_eq!(integer(&engine), 42);
}

#[test]
fn cancellation_discards_pending_publication_and_allows_reuse() {
    let mut engine = Engine::default();
    execute(&mut engine, "let x = 42");
    engine
        .begin(
            &parse(
                "test",
                "let x = 0; let spin = rec { loop () => loop () }; spin ()",
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(engine.step(100).unwrap(), Progress::Yielded);
    let replacement = parse("replacement", "0").unwrap();
    assert_eq!(engine.begin(&replacement).unwrap_err().kind, "RuntimeBusy");
    assert!(engine.interrupt().unwrap_err().is_cancelled());
    engine.collect();
    execute(&mut engine, "x");
    assert_eq!(integer(&engine), 42);
}

#[test]
fn failed_nominal_declaration_does_not_poison_session() {
    let mut engine = Engine::default();
    engine
        .begin(&parse("test", "struct Point {x}; missing").unwrap())
        .unwrap();
    assert_eq!(support::finish(&mut engine).unwrap_err().kind, "NameError");
    execute(&mut engine, "struct Point {x}; (Point {x: 42}).x");
    assert_eq!(integer(&engine), 42);
    engine
        .begin(&parse("test", "struct Point {x}").unwrap())
        .unwrap();
    assert_eq!(support::finish(&mut engine).unwrap_err().kind, "NameError");
}

#[test]
fn retained_functions_keep_definition_source_for_errors() {
    let mut engine = Engine::default();
    let definition = "fn fail x = x + true";
    engine
        .begin(&parse("definition.rill", definition).unwrap())
        .unwrap();
    while engine.step(64).unwrap() != Progress::Complete {}
    engine.collect();
    engine
        .begin(&parse("caller.rill", "fail 42").unwrap())
        .unwrap();
    let error = support::finish(&mut engine).unwrap_err();
    let source = error.origin.unwrap();
    assert_eq!(source.name, "definition.rill");
    assert_eq!(source.text, definition);
}

#[test]
fn nominal_updates_preserve_identity_and_the_original() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        "struct Point {x}; let original = Point {x: 1}; let changed = original with {x: 41}; match changed of { Point {x} => x + original.x }",
    );
    assert_eq!(integer(&engine), 42);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn checked_addition_agrees_with_wider_integer_arithmetic(a: i64, b: i64) {
        let module = parse("arithmetic", &format!("({a}) + ({b})")).unwrap();
        let mut engine = Engine::default();
        engine.begin(&module).unwrap();
        let expected = i64::try_from(i128::from(a) + i128::from(b));
        if let Ok(expected) = expected {
            support::finish(&mut engine).unwrap();
            proptest::prop_assert_eq!(integer(&engine), expected);
        } else {
            proptest::prop_assert_eq!(support::finish(&mut engine).unwrap_err().kind, "ArithmeticError");
        }
    }
}

#[test]
fn constant_loading_is_lazy_and_retained_closures_survive_collection_and_failure() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        r#"
fn choose flag = if flag then {label: "retained", number: 4_2} else 9223372036854775807 + 1
let curry = { ignored flag => choose flag }
let partial = curry ()
"#,
    );
    for _ in 0..3 {
        engine.collect();
        execute(&mut engine, "partial true");
        assert!(engine.inspect(|value| {
            matches!(value.field("number"), Ok(Value::Int(42)))
                && matches!(value.field("label"), Ok(Value::String(label)) if label.as_str() == "retained")
        }));
        engine
            .begin(&parse("caller", "partial false").unwrap())
            .unwrap();
        let error = loop {
            match engine.step(8) {
                Err(error) => break error,
                Ok(Progress::Yielded) => engine.collect(),
                Ok(_) => panic!("overflowing branch unexpectedly completed"),
            }
        };
        assert_eq!(error.kind, "ArithmeticError");
        let source = error.origin.unwrap();
        assert_eq!(&source.text[error.span.unwrap()], "9223372036854775807 + 1");
    }
}

#[test]
fn lexical_slots_preserve_shadowing_and_definition_time_capture() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        r"
let outside = 7
let keep = { value => do {
  let saved = { () => value + outside }
  let value = 100
  let outside = 200
  saved
} }
let saved = keep 35
",
    );
    execute(&mut engine, "let outside = 300");
    engine.collect();
    execute(&mut engine, "saved ()");
    assert_eq!(integer(&engine), 42);

    engine
        .begin(&parse("forward", "let f = { () => later }; let later = 42").unwrap())
        .unwrap();
    assert_eq!(support::finish(&mut engine).unwrap_err().kind, "NameError");
    execute(&mut engine, "saved ()");
    assert_eq!(integer(&engine), 42);
}

#[test]
fn staged_parameter_patterns_can_use_then_shadow_a_capture() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        r"
enum Box {Some {value}}
let kind = Box
let decode = { (kind.Some {value}) kind => value + kind }
let partial = decode (Box.Some {value: 40})
",
    );
    execute(&mut engine, "let kind = 99");
    engine.collect();
    execute(&mut engine, "partial 2");
    assert_eq!(integer(&engine), 42);
}

#[test]
fn nested_mutual_groups_keep_outer_captures_and_partial_arguments() {
    let mut engine = Engine::default();
    execute(
        &mut engine,
        r"
let make = { offset => do {
  rec {
    fn left n extra = if n == 0 then offset + extra else right (n - 1) extra
    fn right n extra = if n == 0 then offset + extra else left (n - 1) extra
  }
  left
} }
let partial = make 40 1000
",
    );
    engine.collect();
    engine.begin(&parse("call", "partial 2").unwrap()).unwrap();
    while engine.step(32).unwrap() != Progress::Complete {
        engine.collect();
    }
    assert_eq!(integer(&engine), 42);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn record_rest_preserves_unmatched_fields_in_source_order(
        fields in proptest::collection::vec((proptest::prelude::any::<i64>(), proptest::prelude::any::<bool>()), 0..64),
    ) {
        let record = fields.iter().enumerate()
            .map(|(index, (value, _))| format!("field_{index}: {value}"))
            .collect::<Vec<_>>().join(", ");
        // Reverse the pattern order so the oracle cannot accidentally follow it.
        let pattern = fields.iter().enumerate().rev()
            .filter(|(_, (_, selected))| *selected)
            .map(|(index, _)| format!("field_{index}: _"))
            .chain(std::iter::once("..rest".into()))
            .collect::<Vec<_>>().join(", ");
        let mut engine = Engine::default();
        execute(&mut engine, &format!("let {{{pattern}}} = {{{record}}}; rest"));
        engine.collect();
        let actual = engine.inspect(|value| {
            let Value::Record(record) = value else { panic!("expected Record") };
            record.iter().map(|(key, value)| {
                let Value::Int(value) = value else { panic!("expected Int") };
                (key.clone(), *value)
            }).collect::<Vec<_>>()
        });
        let expected = fields.iter().enumerate()
            .filter(|(_, (_, selected))| !selected)
            .map(|(index, (value, _))| (format!("field_{index}"), *value))
            .collect::<Vec<_>>();
        proptest::prop_assert_eq!(actual, expected);
    }
}
