//! Observe producer callbacks and cleanup through the ordinary host protocol under stress GC.
use rill_runtime::{
    Engine, Error, Progress,
    host::{Request, Response},
    value::Value,
};

const SUPPORT: &str = r#"
fn reason_name reason = match reason of {
  CloseReason.Exhausted => "exhausted",
  CloseReason.Closed => "closed",
  CloseReason.Cutoff => "cutoff",
  CloseReason.Failed {error} => "failed:" + error.kind,
  CloseReason.Cancelled => "cancelled"
}
fn counting limit = produce {
  acquire: { () => do { print "acquire"; 0 } },
  step: { state => do {
    print ("step " + text state)
    if state < limit then some [state, state + 1] else Option.None
  } },
  release: { state reason => print ("release " + text state + " " + reason_name reason) }
}
"#;

fn run(
    engine: &mut Engine,
    source: &str,
    events: &mut Vec<String>,
    cancel_on: Option<&str>,
) -> Result<(), Error> {
    let module = rill_syntax::parse("producer-test", &format!("{SUPPORT}\n{source}")).unwrap();
    engine.begin(&module)?;
    let mut cancelled = false;
    let mut pending = String::new();
    loop {
        if engine.step(8)? == Progress::Complete {
            return Ok(());
        }
        engine.collect();
        if let Some(request) = engine.take_request() {
            let request = match request {
                Request::Defer(request) => *request,
                request => request,
            };
            let Request::Write(bytes) = request else {
                panic!("unexpected request {request:?}")
            };
            let message = std::str::from_utf8(&bytes).unwrap();
            pending.push_str(message);
            let mut interrupt = false;
            while let Some(end) = pending.find('\n') {
                let line: String = pending.drain(..=end).collect();
                interrupt |= cancel_on == Some(line.trim_end()) && !cancelled;
                events.push(line.trim_end().into());
            }
            if interrupt {
                cancelled = true;
                engine.interrupt()?;
            } else {
                if cancelled && engine.cleaning() {
                    engine.interrupt()?;
                }
                engine.resume(Ok(Response::Unit))?;
            }
        }
    }
}

#[test]
fn acquisition_is_lazy_and_release_observes_the_last_successful_state() {
    let mut engine = Engine::standard().unwrap();
    for source in ["counting 3 |> close", "counting 3 |> take 0 |> collect"] {
        let mut events = Vec::new();
        run(&mut engine, source, &mut events, None).unwrap();
        assert!(events.is_empty());
    }
    let mut events = Vec::new();
    run(&mut engine, "counting 3 |> collect", &mut events, None).unwrap();
    assert_eq!(
        events,
        [
            "acquire",
            "step 0",
            "step 1",
            "step 2",
            "step 3",
            "release 3 exhausted"
        ]
    );
    assert!(
        engine.inspect(|value| matches!(value, Value::List(list) if list.as_slice().len() == 3))
    );
    events.clear();
    run(
        &mut engine,
        "counting 3 |> take 1 |> collect",
        &mut events,
        None,
    )
    .unwrap();
    assert_eq!(events, ["acquire", "step 0", "release 1 cutoff"]);
}

#[test]
fn adjacent_transforms_preserve_demand_effects_failures_and_cleanup() {
    let mut engine = Engine::standard().unwrap();
    let source = r#"
counting 5
  |> map { n => do { print ("map " + text n); n + 1 } }
  |> filter { n => do { print ("filter " + text n); n > 1 } }
  |> take 1
  |> collect
"#;
    let mut events = Vec::new();
    run(&mut engine, source, &mut events, None).unwrap();
    assert_eq!(
        events,
        [
            "acquire",
            "step 0",
            "map 0",
            "filter 1",
            "step 1",
            "map 1",
            "filter 2",
            "release 2 cutoff"
        ]
    );
    assert!(engine.inspect(
        |value| matches!(value, Value::List(list) if matches!(list.as_slice(), [Value::Int(2)]))
    ));
    events.clear();
    let error = run(&mut engine, source, &mut events, Some("map 1")).unwrap_err();
    assert!(error.is_cancelled());
    assert_eq!(
        events,
        [
            "acquire",
            "step 0",
            "map 0",
            "filter 1",
            "step 1",
            "map 1",
            "release 2 cancelled"
        ]
    );
    events.clear();
    let error = run(
        &mut engine,
        "counting 5 |> map { n => n + 1 } |> filter { _ => 42 } |> collect",
        &mut events,
        None,
    )
    .unwrap_err();
    assert_eq!(error.kind, "TypeError");
    assert_eq!(events, ["acquire", "step 0", "release 1 failed:TypeError"]);
}

#[test]
fn failed_step_keeps_state_and_release_errors_are_secondary() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    let error = run(
        &mut engine,
        r#"
produce {
  acquire: { () => 41 },
  step: { state => raise (error "StepFailure" (text state)) },
  release: { state reason => do {
    print (text state + " " + reason_name reason)
    raise (error "ReleaseFailure" "secondary")
  } }
} |> collect
"#,
        &mut events,
        None,
    )
    .unwrap_err();
    assert_eq!(error.kind, "StepFailure");
    assert_eq!(events, ["41 failed:StepFailure"]);
    assert!(
        error
            .notes
            .iter()
            .any(|note| note.contains("ReleaseFailure"))
    );
}

#[test]
fn acquisition_failure_does_not_call_release_and_protocol_validation_is_eager() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    let error = run(
        &mut engine,
        r#"
produce {
  acquire: { () => raise (error "AcquireFailure" "no state") },
  step: identity,
  release: { state reason => print "unexpected release" }
} |> collect
"#,
        &mut events,
        None,
    )
    .unwrap_err();
    assert_eq!(error.kind, "AcquireFailure");
    assert!(events.is_empty());
    for source in [
        "produce {acquire: 1, step: identity, release: identity} |> close",
        "produce {acquire: identity, step: identity, release: identity, extra: 0} |> close",
    ] {
        assert_eq!(
            run(&mut engine, source, &mut events, None)
                .unwrap_err()
                .kind,
            "TypeError"
        );
    }
}

#[test]
fn failed_release_does_not_skip_other_inputs_or_replace_the_primary_error() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    let error = run(
        &mut engine,
        r#"
fn failing name = produce {
  acquire: { () => name },
  step: { state => some [state, state] },
  release: { state reason => do { print state; raise (error "ReleaseFailure" state) } }
}
zip [failing "left", failing "right"] |> take 1 |> collect
"#,
        &mut events,
        None,
    )
    .unwrap_err();
    assert_eq!(events, ["left", "right"]);
    assert_eq!(error.kind, "ReleaseFailure");
    assert_eq!(error.message, "left");
    assert!(error.notes.iter().any(|note| note.contains("right")));
}

#[test]
fn cancellation_releases_once_and_cannot_be_caught_by_attempt() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    let error = run(
        &mut engine,
        "attempt { () => counting 10 |> collect }",
        &mut events,
        Some("step 1"),
    )
    .unwrap_err();
    assert!(error.is_cancelled());
    assert_eq!(
        events,
        ["acquire", "step 0", "step 1", "release 1 cancelled"]
    );
}

#[test]
fn malformed_or_escaping_step_results_keep_the_previous_state() {
    let mut engine = Engine::standard().unwrap();
    for result in ["[0, state]", "some [0]", "some [items [], state]"] {
        let mut events = Vec::new();
        let source = format!(
            r"produce {{
  acquire: {{ () => 7 }},
  step: {{ state => {result} }},
  release: {{ state reason => print (text state) }}
}} |> collect"
        );
        assert!(run(&mut engine, &source, &mut events, None).is_err());
        assert_eq!(events, ["7"]);
    }
}

#[test]
fn caught_failures_keep_the_original_nominal_error_after_failed_release() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    run(
        &mut engine,
        r#"
let result = attempt { () => produce {
  acquire: { () => 0 },
  step: { state => raise (error "StepFailure" "primary") },
  release: { state reason => raise (error "ReleaseFailure" "secondary") }
} |> collect }
match result of { Result.Err {error} => error.kind, _ => "unexpected" }
"#,
        &mut events,
        None,
    )
    .unwrap();
    assert!(
        engine.inspect(
            |value| matches!(value, Value::String(kind) if kind.as_str() == "StepFailure")
        )
    );
}

#[test]
fn producer_scopes_cannot_consume_or_adopt_an_outer_stream() {
    let mut engine = Engine::standard().unwrap();
    for acquisition in ["outer", "do { close outer; 0 }"] {
        let mut events = Vec::new();
        let source = format!(
            r#"do {{
  let outer = items [42]
  let outcome = attempt {{ () => produce {{
    acquire: {{ () => {acquisition} }},
    step: {{ state => Option.None }},
    release: {{ state reason => print "unexpected" }}
  }} |> collect }}
  print (match outcome of {{ Result.Err {{error}} => error.kind, _ => "unexpected" }})
  outer |> sum
}}"#
        );
        run(&mut engine, &source, &mut events, None).unwrap();
        assert_eq!(events, ["ResourceEscape"]);
        assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
    }
}

#[test]
fn release_can_catch_its_own_errors_and_must_finish_with_unit() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    run(
        &mut engine,
        r#"produce {
  acquire: { () => 0 },
  step: { state => Option.None },
  release: { state reason => do {
    let caught = attempt { () => raise (error "Handled" "inside release") }
    print (match caught of { Result.Err {error} => error.kind, _ => "unexpected" })
  } }
} |> collect"#,
        &mut events,
        None,
    )
    .unwrap();
    assert_eq!(events, ["Handled"]);
    assert_eq!(run(&mut engine, "produce {acquire: identity, step: { state => Option.None }, release: { state reason => 1 }} |> collect", &mut events, None).unwrap_err().kind, "TypeError");
}

#[test]
fn cancellation_protects_release_but_keeps_its_local_error_handling() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    let error = run(
        &mut engine,
        r#"
produce {
  acquire: { () => 0 },
  step: { state => do { print "cancel here"; some [state, state + 1] } },
  release: { state reason => do {
    print "release started"
    let outcome = attempt { () => raise (error "Handled" "cleanup callback") }
    print (match outcome of { Result.Err {error} => error.kind, _ => "unexpected" })
    print "release finished"
  } }
} |> collect
"#,
        &mut events,
        Some("cancel here"),
    )
    .unwrap_err();
    assert!(error.is_cancelled());
    assert_eq!(
        events,
        [
            "cancel here",
            "release started",
            "Handled",
            "release finished"
        ]
    );
}

#[test]
fn staged_release_failure_still_finalizes_every_producer() {
    for release in [
        "{ state => raise (error \"ReleaseFailure\" \"first application\") }",
        "{ state => 42 }",
    ] {
        let mut engine = Engine::standard().unwrap();
        let mut events = Vec::new();
        let source = format!(
            r"
do {{
let first = produce {{acquire: {{ () => 0 }}, step: {{ state => some [state, state] }}, release: {release}}}
zip [first, counting 3] |> take 1 |> collect
}}
",
        );
        let error = run(&mut engine, &source, &mut events, None).unwrap_err();
        assert!(matches!(
            error.kind.as_str(),
            "ReleaseFailure" | "TypeError"
        ));
        assert_eq!(events, ["acquire", "step 0", "release 1 cutoff"]);
    }
}

#[test]
fn release_can_run_nested_producers_without_losing_the_parent_continuation() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    run(
        &mut engine,
        r#"
produce {
  acquire: { () => 0 },
  step: { state => some [state, state + 1] },
  release: { state reason => do {
    print "parent release"
    counting 2 |> collect
    print "parent done"
  } }
} |> take 1 |> collect
"#,
        &mut events,
        None,
    )
    .unwrap();
    assert_eq!(
        events,
        [
            "parent release",
            "acquire",
            "step 0",
            "step 1",
            "step 2",
            "release 2 exhausted",
            "parent done"
        ]
    );
}

#[test]
fn cleanup_preserves_the_primary_error_source_location() {
    let mut engine = Engine::standard().unwrap();
    let mut events = Vec::new();
    let error = run(
        &mut engine,
        r#"
fn fail_step state = 1 / 0
produce {
  acquire: { () => 0 },
  step: fail_step,
  release: { state reason => print "released" }
} |> collect
"#,
        &mut events,
        None,
    )
    .unwrap_err();
    let source = error.origin.unwrap();
    assert_eq!(&source.text[error.span.unwrap()], "1 / 0");
    assert_eq!(events, ["released"]);
}

#[test]
fn host_cancellation_uses_the_same_uncatchable_cleanup_path() {
    let mut engine = Engine::standard().unwrap();
    let module = rill_syntax::parse("host-cancel", r#"
attempt { () => produce {
  acquire: { () => 0 },
  step: { state => do { print "interrupt here"; Option.None } },
  release: { state reason => print (match reason of { CloseReason.Cancelled => "released", _ => "unexpected" }) }
} |> collect }
"#).unwrap();
    engine.begin(&module).unwrap();
    let mut cancelled = false;
    let mut output = Vec::new();
    loop {
        match engine.step(8) {
            Err(error) => {
                assert!(error.is_cancelled());
                assert_eq!(output, b"released\n");
                return;
            }
            Ok(Progress::Complete) => panic!("attempt caught host cancellation"),
            Ok(_) => {}
        }
        engine.collect();
        if let Some(request) = engine.take_request() {
            let request = match request {
                Request::Defer(request) => *request,
                request => request,
            };
            let Request::Write(bytes) = request else {
                panic!("unexpected host request")
            };
            if cancelled {
                let replacement = rill_syntax::parse("replacement", "99").unwrap();
                assert_eq!(engine.begin(&replacement).unwrap_err().kind, "RuntimeBusy");
                output.extend(bytes);
                engine.resume(Ok(Response::Unit)).unwrap();
            } else {
                cancelled = true;
                engine
                    .resume(Err(Error::cancelled("host interruption")))
                    .unwrap();
            }
        }
    }
}
