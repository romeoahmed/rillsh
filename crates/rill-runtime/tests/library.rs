//! Bundled library contracts, independent value models and cleanup under collection.
mod support;
use proptest::strategy::Strategy;
use rill_runtime::{Engine, Progress, value::Value};
use support::run;

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
        r#"from_json_with {max_bytes: 1} "[]""#,
        "to_json_with {max_bytes: 1} []",
        r#"from_json_with {max_depth: 1} "[0]""#,
        "to_json_with {max_depth: 1} [0]",
    ] {
        assert_eq!(run(&mut engine, source).unwrap_err().kind, "LimitExceeded");
    }
    let error = run(&mut engine, "to_json {data: [some 1]}").unwrap_err();
    assert_eq!(error.kind, "TypeError");
    assert!(error.message.contains("$[\"data\"][0]"));
    let nested = format!("{}0{}", "[".repeat(300), "]".repeat(300));
    run(
        &mut engine,
        &format!("from_json_with {{max_depth: 301}} '{nested}'"),
    )
    .unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::List(_))));
}

#[test]
fn sorting_is_stable_and_checks_input_before_callbacks() {
    let mut engine = Engine::standard().unwrap();
    run(&mut engine, r#"[{key: 2, id: "a"}, {key: 1, id: "b"}, {key: 2, id: "c"}] |> sort_by { item => item.key } |> map { item => item.id } |> equal ["b", "a", "c"]"#).unwrap();
    assert!(engine.inspect(|v| matches!(v, Value::Bool(true))));
    assert_eq!(
        run(
            &mut engine,
            "sort_by_with {max_items: 0} { _ => raise (error \"Called\" \"bad\") } [1]"
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
        run(&mut engine, "sort_by_with {max_bytes: 0} identity [1]")
            .unwrap_err()
            .kind,
        "LimitExceeded"
    );
    run(
        &mut engine,
        "sort_by_with {max_bytes: 0, max_items: 0} identity [] == []",
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
    run(&mut engine, r#"let name = "hello world"; job { ^printf "%s" $name ...$(["", "tail"]) 2>&1 > "output" | ^cat }"#).unwrap();
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
        r#"job { ^echo $(42) $(raise (error "Late" "must not execute")) }"#,
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
    run(&mut engine, r#"job { ^echo $(match attempt { () => job { ^cat $(false) } } of { Result.Err {error: _} => "recovered", _ => "wrong" }) }"#).unwrap();
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
        ("collect_with {max_items: 2} (range 0 3)", "LimitExceeded"),
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
            r#"chunks (encode_utf8 "abc\n") |> lines_with {max_line_bytes: 2} |> collect"#,
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
        r#"join "" (scalars "a\u{1f30a}\u{301}") == "a\u{1f30a}\u{301}""#,
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
fn materialization_accounts_for_constructor_descriptions() {
    let mut engine = Engine::standard().unwrap();
    let name = "x".repeat(8192);
    let source =
        format!("struct Long {{{name}}}; items [Long] |> collect_with {{max_bytes: 1024}}");
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
