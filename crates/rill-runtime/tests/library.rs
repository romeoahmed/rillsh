//! Bundled library contracts, independent value models and cleanup under collection.
mod support;
use proptest::strategy::Strategy;
use rill_runtime::{Engine, Progress, value::Value};
use support::run;

#[test]
fn invalid_environment_names_fail_before_host_effects() {
    let mut engine = Engine::standard().unwrap();
    for name in [r#""""#, r#""a=b""#, r#""a\u{0}b""#, "bytes [65]", "42"] {
        for source in [
            format!("get_env ({name})"),
            format!("set_env ({name}) \"value\""),
            format!("unset_env ({name})"),
            format!("without_env [{name}] (plan {{ ^true }})"),
        ] {
            assert_eq!(
                run(&mut engine, &source).unwrap_err().kind,
                "TypeError",
                "{source}"
            );
        }
    }
    for name in [r#""""#, r#""a=b""#, r#""a\u{0}b""#] {
        let source = format!("with_env {{{name}: \"value\"}} (plan {{ ^true }})");
        assert_eq!(run(&mut engine, &source).unwrap_err().kind, "TypeError");
    }
}

#[test]
fn bundled_composition_and_list_callbacks_use_the_shared_call_protocol() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        "[1, 2, 3] |> map (add 1) |> filter { x => x > 2 } |> sum",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Int(7))));
    run(&mut engine, "fold_until { acc item => if item == 3 then Control.Stop {value: acc} else Control.Continue {value: acc + item} } 0 [1, 2, 3, 4]").unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Int(3))));
}

#[test]
fn attempt_survives_nested_tail_calls_and_gc() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "fn recur n = if n == 0 then 1 + true else recur (n - 1); match attempt { () => recur 1000 } of { Result.Err {error} => error.kind, _ => \"bad\" }").unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::String(s) if s.as_str() == "TypeError")));
    run(
        &mut engine,
        "fn go () = attempt { () => 42 }; match go () of { Result.Ok {value} => value, _ => 0 }",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Int(42))));
    run(
        &mut engine,
        "attempt { () => attempt { () => raise (error \"Custom\" \"message\") } }",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Adt(a) if a.descriptor.name == "Result.Ok")));
}

#[test]
fn option_and_result_modules_share_nominal_descriptors() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "import \"std:option\" as option; import \"std:result\" as result; let a = some 20 |> option.map (add 1) |> option.unwrap_or 0; let b = ok 20 |> result.map (add 1) |> result.unwrap_or 0; a + b").unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Int(42))));
}

#[test]
fn bind_preserves_container_identity_and_skips_absence_or_failure() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        r#"
      import "std:option" as option
      import "std:result" as result
      fn unused _ = raise (error "Unexpected" "callback must be skipped")
      let failure = err "original"
      ((option.bind unused Option.None == Option.None)
        and (result.bind unused failure == failure)
        and (option.bind { x => some (x + 1) } (some 2) == some 3)
        and (result.bind { x => ok (x + 1) } (ok 2) == ok 3)
        and (option.bind { _ => Option.None } (some 2) == Option.None)
        and (result.bind { _ => failure } (ok 2) == failure))
    "#,
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    for source in [
        "option.bind identity (some 1)",
        "result.bind identity (ok 1)",
        "option.bind ok (some 1)",
        "result.bind some (ok 1)",
        "option.bind { x => {value: x} } (some 1)",
        "enum Other {Some {value}}; option.bind { x => Other.Some {value: x} } (some 1)",
    ] {
        assert_eq!(
            run(&mut engine, source).unwrap_err().kind,
            "TypeError",
            "{source}"
        );
    }
    run(&mut engine, "option.map some (some 1) == some (some 1)").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
}

#[test]
fn prelude_namespaces_share_exports_and_keep_specialized_operations_qualified() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        r#"
      import "std:seq" as sequence
      import "std:text" as strings
      let data = items [1, 2] |> seq.collect_with {max_items: 2}
      ((sequence.CloseReason.Closed == seq.CloseReason.Closed)
        and (data == [1, 2])
        and (string 42 == "42")
        and (core.string true == "true")
        and (strings.byte_length "abc" == text.byte_length "abc")
        and (fs.basename "dir/file" == path "file")
        and (json.from_json "42" == 42))
    "#,
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    for name in [
        "collect_with",
        "produce",
        "CloseReason",
        "byte_length",
        "scalars",
        "path_bytes",
    ] {
        assert_eq!(
            run(&mut engine, name).unwrap_err().kind,
            "NameError",
            "{name}"
        );
    }
}

#[test]
fn byte_text_and_paths_preserve_distinct_kinds() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        "decode_utf8 (encode_utf8 \"a\\u{1f30a}\\u{0}\") == \"a\\u{1f30a}\\u{0}\"",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    run(
        &mut engine,
        "path (bytes [255, 47, 120]) == path (bytes [255, 47, 120])",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    assert_eq!(
        run(&mut engine, "basename \"a\\u{0}/b\"").unwrap_err().kind,
        "TypeError"
    );
    assert_eq!(
        run(&mut engine, "decode_utf8 (bytes [255])")
            .unwrap_err()
            .kind,
        "DecodeError"
    );
}

#[test]
fn numeric_text_parsing_and_conversion_check_boundaries() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        "int (-3.9) == -3 and div (-7) 3 == -2 and rem (-7) 3 == -1",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    for source in [
        "parse_int \"+1\"",
        "parse_int \"01\"",
        "parse_float \" 1\"",
        "parse_float \"NaN\"",
        "parse_int \"1e2\"",
    ] {
        assert_eq!(run(&mut engine, source).unwrap_err().kind, "DecodeError");
    }
    for source in [
        "parse_int \"9223372036854775808\"",
        "parse_float \"1e999\"",
        "int 9223372036854775808.0",
        "div (-9223372036854775808) (-1)",
    ] {
        assert_eq!(
            run(&mut engine, source).unwrap_err().kind,
            "ArithmeticError"
        );
    }
}

#[test]
fn record_primitives_preserve_field_order_and_reject_duplicates() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "let base = {b: 2, a: 1}; record (entries base) == base and entries base == [[\"b\", 2], [\"a\", 1]]").unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    assert_eq!(
        run(&mut engine, "record [[\"x\", 1], [\"x\", 2]]")
            .unwrap_err()
            .kind,
        "TypeError"
    );
}

#[test]
fn json_preserves_numeric_kinds_and_treats_library_marker_keys_as_data() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"let value = {"$serde_json::private::Number": "123", data: [0, -0.0, 1.0, 9223372036854775807, null]}; from_json (to_json value) == value"#).unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    run(&mut engine, r#"from_json "-0" == 0"#).unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    for source in [
        r#"from_json '{"x": 1, "x": 2}'"#,
        r#"from_json '{"x": 1, "\u0078": 2}'"#,
        r#"from_json '{"outer": {"x": 1, "x": 2}}'"#,
        r#"from_json "[1,]""#,
    ] {
        assert_eq!(run(&mut engine, source).unwrap_err().kind, "DecodeError");
    }
    for source in [r#"from_json "9223372036854775808""#, r#"from_json "1e999""#] {
        assert_eq!(
            run(&mut engine, source).unwrap_err().kind,
            "ArithmeticError"
        );
    }
}

#[test]
fn json_limits_do_not_publish_truncated_success() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        r#"json.from_json_with {max_bytes: 1} "[]""#,
        "json.to_json_with {max_bytes: 1} []",
        r#"json.from_json_with {max_depth: 1} "[0]""#,
        "json.to_json_with {max_depth: 1} [0]",
    ] {
        assert_eq!(run(&mut engine, source).unwrap_err().kind, "LimitExceeded");
    }
    let error = run(&mut engine, "to_json {data: [some 1]}").unwrap_err();
    assert_eq!(error.kind, "TypeError");
    assert!(error.message.contains("$[\"data\"][0]"));
    let nested = format!("{}0{}", "[".repeat(300), "]".repeat(300));
    run(
        &mut engine,
        &format!("json.from_json_with {{max_depth: 301}} '{nested}'"),
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::List(_))));
}

#[test]
fn json_error_paths_follow_nested_values_after_completed_siblings() {
    let mut engine = Engine::standard().unwrap();
    for (source, kind, path) in [
        ("to_json identity", "TypeError", "$"),
        (
            r#"to_json [{done: [1, 2]}, {"a\"\n": [0, some 1]}]"#,
            "TypeError",
            r#"$[1]["a\"\n"][1]"#,
        ),
        (
            "json.to_json_with {max_depth: 3} {done: [1], later: [[0]]}",
            "LimitExceeded",
            r#"$["later"][0][0]"#,
        ),
    ] {
        engine
            .begin(&rill_syntax::parse("json-path", source).unwrap())
            .unwrap();
        let error = loop {
            match engine.step(1) {
                Err(error) => break error,
                Ok(Progress::Yielded) => engine.collect(),
                progress => panic!("expected encoding failure, got {progress:?}"),
            }
        };
        assert_eq!(error.kind, kind);
        assert!(
            error.message.ends_with(&format!("at {path}")),
            "{}",
            error.message
        );
    }
}

#[test]
fn sorting_is_stable_and_checks_input_before_callbacks() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"[{key: 2, id: "a"}, {key: 1, id: "b"}, {key: 2, id: "c"}] |> sort_by { item => item.key } |> map { item => item.id } |> equal ["b", "a", "c"]"#).unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    assert_eq!(
        run(
            &mut engine,
            "seq.sort_by_with {max_items: 0} { _ => raise (error \"Called\" \"bad\") } [1]"
        )
        .unwrap_err()
        .kind,
        "LimitExceeded"
    );
    assert_eq!(
        run(&mut engine, "sort_by identity [1, 2.0]")
            .unwrap_err()
            .kind,
        "TypeError"
    );
    assert_eq!(
        run(&mut engine, "seq.sort_by_with {max_bytes: 0} identity [1]")
            .unwrap_err()
            .kind,
        "LimitExceeded"
    );
    run(
        &mut engine,
        "seq.sort_by_with {max_bytes: 0, max_items: 0} identity [] == []",
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
}

#[test]
fn lexical_paths_do_not_normalize_components_or_byte_identity() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"path "a//b" != path "a/b" and basename "a/." == path "." and basename "" == path "." and basename "///" == path "/" and dirname "a/b///" == path "a" and join_path "a" "" == path "a""#).unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
}

#[test]
fn plans_are_evaluated_once_and_keep_argument_and_redirect_order() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"let name = "hello world"; plan { ^printf "%s" $name ...$(["", "tail"]) 2>&1 > "output" | ^cat }"#).unwrap();
    engine.inspect(|value| {
        let Value::Plan(plan) = value else {
            panic!("expected JobPlan")
        };
        assert_eq!(plan.0.stages.len(), 2);
        let argv: Vec<_> = plan.0.stages[0].argv.iter().map(|s| s.to_bytes()).collect();
        assert_eq!(
            argv,
            [b"printf".as_slice(), b"%s", b"hello world", b"", b"tail"]
        );
        assert!(matches!(
            plan.0.stages[0].redirects[0],
            rill_system::plan::Redirect::ErrorToOutput
        ));
    });
    let error = run(
        &mut engine,
        r#"plan { ^echo $(42) $(raise (error "Late" "must not execute")) }"#,
    )
    .unwrap_err();
    assert_eq!(error.kind, "TypeError");
    assert!(error.message.contains("argv[1]"));
    run(&mut engine, r#"let first = command "printf" ["hi"]; let both = pipe first (command "cat" []); accept_exit [0, 1] both"#).unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Plan(plan) if plan.0.stages.len() == 2 && plan.0.stages[0].accepted_codes == [0] && plan.0.stages[1].accepted_codes == [0, 1])));
}

#[test]
fn failed_nested_plan_construction_does_not_corrupt_outer_builder() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"plan { ^echo $(match attempt { () => plan { ^cat $(false) } } of { Result.Err {error: _} => "recovered", _ => "wrong" }) }"#).unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Plan(plan) if plan.0.stages[0].argv[1].to_bytes() == b"recovered")));
}

#[test]
fn lazy_callbacks_remain_rooted_across_every_quantum() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        "range 0 100 |> filter { n => rem n 2 == 0 } |> map (multiply 3) |> take 4 |> sum",
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(36))));
    run(&mut engine, "range 0 10 |> drop 7 |> collect |> to_json").unwrap();
    assert!(
        engine.inspect(
            |value| matches!(value, Value::Bytes(bytes) if bytes.0.as_ref() == b"[7,8,9]")
        )
    );
}

#[test]
fn cutoff_does_not_invoke_an_unneeded_callback() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"unfold { _ => raise (error "Unreachable" "must not run") } () |> take 0 |> collect |> length"#).unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(0))));
    run(
        &mut engine,
        r#"
      unfold { n =>
        if n == 4 then raise (error "Unreachable" "pulled past cutoff")
        else some [n, n + 1]
      } 0
        |> fold_until { acc n =>
          if n == 2 then Control.Stop {value: acc}
          else Control.Continue {value: acc + n}
        } 0
    "#,
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(1))));
}

#[test]
fn invalid_callback_does_not_transfer_a_valid_source() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "do { let source = items [1, 2]; attempt { () => map 42 source }; collect source |> length }").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(2))));
}

#[test]
fn local_nested_streams_do_not_gain_a_persistent_lifetime() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "do { let inner = items [1, 2]; let outer = collect (items [inner]); collect outer[0] |> length }").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(2))));
    let error = run(
        &mut engine,
        "do { let inner = items [1]; collect (items [inner]) }",
    )
    .unwrap_err();
    assert_eq!(error.kind, "ResourceEscape");
}

#[test]
fn consumer_errors_are_caught_without_publishing_partial_data() {
    let mut engine = Engine::standard().unwrap();
    for (source, expected) in [
        (
            "seq.collect_with {max_items: 2} (range 0 3)",
            "LimitExceeded",
        ),
        ("collect (filter { _ => 1 } (items [1]))", "TypeError"),
        ("collect_bytes (items [1])", "TypeError"),
    ] {
        run(&mut engine, &format!("match attempt {{ () => {source} }} of {{ Result.Err {{error}} => error.kind, _ => \"bad\" }}")).unwrap();
        assert!(
            engine
                .inspect(|value| matches!(value, Value::String(kind) if kind.as_str() == expected))
        );
    }
}

#[test]
fn lines_decode_across_chunks_without_losing_empty_or_unterminated_lines() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        r#"items [bytes [240, 159], bytes [140, 138, 13], bytes [10, 10, 120, 13]] |> lines |> collect |> equal ["\u{1f30a}", "", "x\r"]"#,
        r#"chunks (encode_utf8 "a\n") |> lines |> collect |> equal ["a"]"#,
        "chunks (bytes []) |> lines |> collect |> equal []",
    ] {
        run(&mut engine, source).unwrap();
        assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    }
    for (source, kind) in [
        (
            "chunks (bytes [240, 159]) |> lines |> collect",
            "DecodeError",
        ),
        (
            r#"chunks (encode_utf8 "abc\n") |> text.lines_with {max_line_bytes: 2} |> collect"#,
            "LimitExceeded",
        ),
        ("items [42] |> lines |> collect", "TypeError"),
    ] {
        assert_eq!(run(&mut engine, source).unwrap_err().kind, kind);
    }
}

#[test]
fn zip_validates_all_inputs_before_transfer_and_stops_at_the_shortest() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        "zip [items [1, 2], items [3]] |> collect |> equal [[1, 3]]",
        "zip [] |> collect |> equal []",
        "do { let a = items [1]; attempt { () => zip [a, a] }; collect a == [1] }",
        "do { let a = items [1]; attempt { () => zip [a, 42] }; collect a == [1] }",
        r#"zip [items [], unfold { _ => raise (error "Unreachable" "must not pull") } ()] |> collect |> equal []"#,
    ] {
        run(&mut engine, source).unwrap();
        assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    }
}

#[test]
fn merge_transfers_atomically_and_preserves_each_input_order() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        "merge [] |> collect |> equal []",
        "merge [items [], items [1, 2], items []] |> collect |> equal [1, 2]",
        "do { let a = items [1]; attempt { () => merge [a, a] }; collect a == [1] }",
        "do { let a = items [1]; attempt { () => merge [a, 42] }; collect a == [1] }",
        r#"merge [items [1], unfold { _ => raise (error "Unreachable" "cutoff") } ()] |> take 1 |> collect |> equal [1]"#,
        r#"do {
          let output = merge [
            range 0 20 |> map { n => ["left", n] },
            merge [items [], range 30 40] |> map { n => ["right", n] }
          ] |> collect
          let left = output |> filter { [side, _] => side == "left" } |> map { [_, n] => n }
          let right = output |> filter { [side, _] => side == "right" } |> map { [_, n] => n }
          left == collect (range 0 20) and right == collect (range 30 40)
        }"#,
    ] {
        run(&mut engine, source).unwrap();
        assert!(
            engine.inspect(|value| matches!(value, Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn check_uses_the_report_failure_and_nominal_completion_types() {
    let mut engine = Engine::standard().unwrap();
    let error = run(&mut engine, r"
      let report = JobReport {
        id: 1,
        stages: [{index: 0, termination: Termination.Exited {code: 0}, accepted_codes: [0], expected_cutoff: false}],
        completion: Completion.Finished,
        failure: some 0
      }
      check report
    ").expect_err("a present failure must remain a failure");
    assert_eq!(error.exit_status, Some(1));
    assert_eq!(
        run(
            &mut engine,
            "check (JobReport {id: 1, stages: [], completion: (), failure: Option.None})"
        )
        .unwrap_err()
        .kind,
        "TypeError"
    );
    assert_eq!(run(&mut engine, r#"check (JobReport {id: 1, stages: [], completion: Completion.Cancelled {reason: "cancelled"}, failure: Option.None})"#).unwrap_err().kind, "ProcessError");
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn list_flat_map_matches_ordered_concatenation(
        lists in proptest::collection::vec(proptest::collection::vec(-100_i64..100, 0..8), 0..12),
    ) {
        let mut engine = Engine::standard().unwrap();
        run(&mut engine, &format!("{lists:?} |> flat_map identity")).unwrap();
        let expected: Vec<_> = lists.into_iter().flatten().collect();
        engine.inspect(|value| {
            let Value::List(actual) = value else { panic!("expected List") };
            assert_eq!(actual.as_slice().len(), expected.len());
            for (actual, expected) in actual.as_slice().iter().zip(expected) {
                assert!(matches!(actual, Value::Int(n) if *n == expected));
            }
        });
    }

    #[test]
    fn aggregation_preserves_first_keys_and_items(
        values in proptest::collection::vec(-20_i64..20, 0..40),
    ) {
        // A small linear model keeps the oracle independent of the runtime's hash tables.
        let mut groups: Vec<(String, Vec<i64>)> = Vec::new();
        for &value in &values {
            let key = (value % 5).to_string();
            if let Some((_, group)) = groups.iter_mut().find(|(name, _)| name == &key) {
                group.push(value);
            } else {
                groups.push((key, vec![value]));
            }
        }
        let counts: Vec<_> = groups.iter().map(|(key, items)| (key, items.len())).collect();
        let unique: Vec<_> = groups.iter().map(|(_, items)| items[0]).collect();
        let expected = serde_json::to_string(&(&groups, counts, unique)).unwrap();
        let source = format!(r"
let values = {values:?}
let key = {{ value => string (rem value 5) }}
[
  entries (group_by key values),
  entries (count_by key (items values)),
  unique_by key values
] == {expected}
");
        let mut engine = Engine::standard().unwrap();
        run(&mut engine, &source).unwrap();
        proptest::prop_assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    }

    #[test]
    fn line_decoding_is_independent_of_chunk_boundaries(
        data in proptest::prop_oneof![
            proptest::collection::vec(proptest::prelude::any::<u8>(), 0..128),
            "[a-z\\r\\n\\x{1f30a}]{0,64}".prop_map(String::into_bytes),
        ],
        chunk_size in 1_usize..18,
    ) {
        use std::fmt::Write;
        let mut source = String::from("items [bytes [],");
        for chunk in data.chunks(chunk_size) {
            source.push_str("bytes [");
            for byte in chunk { write!(source, "{byte},").unwrap(); }
            source.push_str("],");
        }
        source.push_str("] |> lines |> collect");
        let mut engine = Engine::standard().unwrap();
        let result = run(&mut engine, &source);
        if let Ok(text) = std::str::from_utf8(&data) {
            result.unwrap();
            let expected: Vec<_> = text.lines().collect();
            engine.inspect(|value| {
                let Value::List(lines) = value else { panic!("line decoder returned a non-list") };
                assert_eq!(lines.as_slice().len(), expected.len());
                for (value, text) in lines.as_slice().iter().zip(expected) {
                    assert!(matches!(value, Value::String(line) if line.as_str() == text));
                }
            });
        } else {
            proptest::prop_assert_eq!(result.unwrap_err().kind, "DecodeError");
        }
    }
}

#[test]
fn native_conversions_keep_their_inputs_and_partial_outputs_alive_across_gc() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        r#"join ":" (split ":" "a::b:") == "a::b:""#,
        r#"join "" (text.scalars "a\u{1f30a}\u{301}") == "a\u{1f30a}\u{301}""#,
        "(concat [1, 2] [3, 4] |> reverse |> take 3) == [4, 3, 2]",
        "record (entries {a: 1, b: [2, 3]}) == {a: 1, b: [2, 3]}",
        r#"from_json (to_json {a: [null, true, 1, 2.0], b: "\u{1f30a}"}) == {a: [null, true, 1, 2.0], b: "\u{1f30a}"}"#,
    ] {
        engine
            .begin(&rill_syntax::parse("native-roots", source).unwrap())
            .unwrap();
        loop {
            let progress = engine.step(1).unwrap();
            engine.collect();
            if progress == Progress::Complete {
                break;
            }
        }
        assert!(
            engine.inspect(|value| matches!(value, Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn allocation_heavy_native_work_yields_and_survives_collection() {
    let text = "x".repeat(10_000);
    let json = serde_json::to_string(&vec!["x"; 10_000]).unwrap();
    for source in [
        format!("text.scalars '{text}'"),
        format!("from_json '{json}'"),
    ] {
        let mut engine = Engine::standard().unwrap();
        engine.collect();
        engine
            .begin(&rill_syntax::parse("gc-pressure", &source).unwrap())
            .unwrap();
        // Fuel comfortably exceeds this finite conversion. Allocation pressure must
        // still return control before materializing the entire result in one quantum.
        assert_eq!(engine.step(1_000_000).unwrap(), Progress::Yielded);
        engine.collect();
        support::finish(&mut engine).unwrap();
        engine.inspect(|value| {
            let Value::List(items) = value else {
                panic!("converted List");
            };
            assert_eq!(items.as_slice().len(), 10_000);
            assert!(
                items
                    .as_slice()
                    .iter()
                    .all(|value| matches!(value, Value::String(text) if text.as_str() == "x"))
            );
        });
    }
}

#[test]
fn materialization_accounts_for_constructor_descriptions() {
    let mut engine = Engine::standard().unwrap();
    let name = "x".repeat(8192);
    let source =
        format!("struct Long {{{name}}}; items [Long] |> seq.collect_with {{max_bytes: 1024}}");
    assert_eq!(run(&mut engine, &source).unwrap_err().kind, "LimitExceeded");
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::WithSource("proptest-regressions"))),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn json_objects_preserve_source_order_and_integer_values(
        values in proptest::collection::vec(proptest::prelude::any::<i64>(), 0..40),
    ) {
        // Reverse the keys so sorting them cannot accidentally satisfy the oracle.
        let pairs: Vec<_> = values.iter().enumerate().rev()
            .map(|(index, value)| (format!("field_{index}"), *value)).collect();
        let json = format!("{{{}}}", pairs.iter()
            .map(|(key, value)| format!("\"{key}\":{value}"))
            .collect::<Vec<_>>().join(","));
        let mut engine = Engine::standard().unwrap();
        run(&mut engine, &format!("from_json '{json}'")).unwrap();
        engine.inspect(|value| {
            let Value::Record(fields) = value else { panic!("decoded Record"); };
            let actual: Vec<_> = fields.iter().map(|(name, value)| {
                let Value::Int(value) = value else { panic!("decoded Int"); };
                (name.clone(), *value)
            }).collect();
            assert_eq!(actual, pairs);
        });
    }

    #[test]
    fn sorting_preserves_equal_key_order_across_runs(keys in proptest::collection::vec(-8_i64..8, 0..200)) {
        let mut expected: Vec<_> = keys.iter().copied().enumerate().collect();
        expected.sort_by_key(|(_, key)| *key);
        let source = format!("{} |> sort_by {{ pair => pair[1] }}", serde_json::to_string(&keys.iter().enumerate().collect::<Vec<_>>()).unwrap());
        let mut engine = Engine::standard().unwrap();
        run(&mut engine, &source).unwrap();
        engine.inspect(|value| {
            let Value::List(items) = value else { panic!("sorted List"); };
            let actual: Vec<_> = items.as_slice().iter().map(|value| {
                let Value::List(pair) = value else { panic!("sort pair"); };
                let [Value::Int(index), Value::Int(key)] = pair.as_slice() else { panic!("integer pair"); };
                (usize::try_from(*index).unwrap(), *key)
            }).collect();
            assert_eq!(actual, expected);
        });
    }
}

#[test]
fn flat_map_preserves_order_for_lists_and_lazy_nested_streams() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        "[1, 2, 3] |> flat_map { n => [n, n + 10] }",
        "items [1, 2, 3] |> flat_map { n => items [n, n + 10] } |> collect",
        "items [1, 2, 3] |> flat_map { n => [n, n + 10] } |> collect",
        "range 1 4 |> flat_map { n => range n (n + 1) |> flat_map { x => [x, x + 10] } } |> collect",
    ] {
        run(&mut engine, &format!("({source}) == [1, 11, 2, 12, 3, 13]")).unwrap();
        assert!(
            engine.inspect(|value| matches!(value, Value::Bool(true))),
            "{source}"
        );
    }
    run(&mut engine, "range 0 100 |> flat_map { n => if n == 1 then raise (error \"Unwanted\" \"overread\") else range 0 100 } |> take 2 |> collect").unwrap();
    run(
        &mut engine,
        "items [1, 2] |> flat_map { _ => [] } |> collect",
    )
    .unwrap();
    assert!(
        engine
            .inspect(|value| matches!(value, Value::List(values) if values.as_slice().is_empty()))
    );
}

#[test]
fn nul_records_preserve_bytes_across_chunks_and_enforce_limits() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "(items [bytes [255], bytes [0, 0, 13], bytes [0, 120]] |> split_nul |> collect) == [bytes [255], bytes [], bytes [13], bytes [120]]").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
    assert_eq!(
        run(
            &mut engine,
            "chunks (bytes [1, 2, 0]) |> text.split_nul_with {max_record_bytes: 1} |> collect"
        )
        .unwrap_err()
        .kind,
        "LimitExceeded"
    );
}

#[test]
fn rethrow_preserves_original_span_and_structured_details() {
    let mut engine = Engine::standard().unwrap();
    let error = run(
        &mut engine,
        r"match attempt { () => 1 + true } of {
      Result.Err {error} => raise error,
      Result.Ok {value} => value
    }",
    )
    .unwrap_err();
    let source = error.origin.unwrap();
    assert_eq!(&source.text[error.span.unwrap()], "1 + true");
    run(&mut engine, r#"let original = error "Example" "message" with {details: {stage: 2}, exit_status: 7}; match attempt { () => raise original } of { Result.Err {error} => error == original, _ => false }"#).unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
}

#[test]
fn raising_malformed_errors_does_not_hide_invalid_fields() {
    let mut engine = Engine::standard().unwrap();
    for fields in [
        "{notes: [1]}",
        "{span: false}",
        "{span: {source: 'file', text: 'abc', offset: -1, length: 1}}",
        "{span: {source: 'file', text: 'abc', offset: 2, length: 2}}",
        r#"{span: {source: 'file', text: "\u{e9}", offset: 1, length: 1}}"#,
        "{details: {stage: true}}",
        "{exit_status: 0}",
        "{exit_status: 256}",
    ] {
        let source = format!("raise (error 'Original' 'message' with {fields})");
        assert_eq!(
            run(&mut engine, &source).unwrap_err().kind,
            "TypeError",
            "{source}"
        );
    }
    run(&mut engine, "match attempt { () => raise (error 'Valid' 'message' with {notes: ['first', 'second']}) } of { Result.Err {error} => error.notes == ['first', 'second'], _ => false }").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
}

#[test]
fn finite_aggregations_preserve_key_and_item_order() {
    let mut engine = Engine::standard().unwrap();
    for source in [
        r#"let rows = [{k: "b", n: 1}, {k: "a", n: 2}, {k: "b", n: 3}]; let key = { row => row.k }; group_by key rows == {b: [rows[0], rows[2]], a: [rows[1]]}"#,
        r#"entries (count_by identity (items ["b", "a", "b"])) == [["b", 2], ["a", 1]]"#,
        r#"unique_by { row => row.k } (items [{k: "b", n: 1}, {k: "b", n: 2}, {k: "a", n: 3}]) == [{k: "b", n: 1}, {k: "a", n: 3}]"#,
        "([1, 2] |> flat_map { n => [n, n + 10] } |> sum) == 26",
        "(range 1 3 |> flat_map { n => range n (n + 2) } |> collect) == [1, 2, 2, 3]",
        r#"contains "." "a.b" and replace "." "-" "a.b.c" == "a-b-c""#,
        "text.path_bytes (path (bytes [255, 47, 120])) == bytes [255, 47, 120]",
    ] {
        run(&mut engine, source).unwrap();
        assert!(
            engine.inspect(|value| matches!(value, Value::Bool(true))),
            "{source}"
        );
    }
    assert_eq!(
        run(&mut engine, "group_by identity [1]").unwrap_err().kind,
        "TypeError"
    );
}

#[test]
fn documentation_survives_aliases_and_partial_application() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, "## Add two values.\nfn documented first second = first + second\nlet alias = documented 1\nhelp alias").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::String(text) if text.contains("Add two values.") && text.contains("second"))));
    run(&mut engine, r#"import "std:test" as test; test.assert_equal 42 (add 40 2); test.assert_error "TypeError" { () => 1 + true }; test.assert true"#).unwrap();
    assert_eq!(
        run(
            &mut engine,
            r#"import "std:test" as test; test.assert false"#
        )
        .unwrap_err()
        .kind,
        "AssertionError"
    );
}

#[test]
fn library_failures_identify_the_user_application() {
    let mut engine = Engine::standard().unwrap();
    let error = run(&mut engine, "parse_int 'not a number'").unwrap_err();
    let source = error.origin.unwrap();
    assert_eq!(source.name, "test");
    assert_eq!(
        &source.text[error.span.unwrap()],
        "parse_int 'not a number'"
    );
}

#[test]
fn dynamically_created_merges_preserve_each_inner_before_the_next() {
    let mut engine = Engine::standard().unwrap();
    run(
        &mut engine,
        "range 0 20 |> flat_map { n => merge [items [n], items [n + 100]] } |> collect",
    )
    .unwrap();
    engine.inspect(|value| {
        let Value::List(list) = value else {
            panic!("expected List")
        };
        assert_eq!(list.as_slice().len(), 40);
        for (index, pair) in list.as_slice().as_chunks::<2>().0.iter().enumerate() {
            let mut actual: Vec<_> = pair
                .iter()
                .map(|value| match value {
                    Value::Int(n) => *n,
                    _ => panic!("expected Int"),
                })
                .collect();
            actual.sort_unstable();
            let index = i64::try_from(index).unwrap();
            assert_eq!(actual, [index, index + 100]);
        }
    });
    run(&mut engine, "range 0 3 |> flat_map { n => merge [range n (n + 2), range n (n + 3)] } |> take 1 |> collect").unwrap();
}
