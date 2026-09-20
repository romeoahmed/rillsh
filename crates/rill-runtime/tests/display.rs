//! Interactive stream display participates in evaluation, cleanup and publication.
use rill_runtime::{Engine, Error, Progress, host, value::Value};

fn display(engine: &mut Engine, source: &str, fail: bool) -> Result<Vec<String>, Error> {
    let module = rill_syntax::parse("display-test", source).unwrap();
    engine.begin_interactive_at(
        &module,
        rill_system::source::Directory::open(std::path::Path::new(".")).unwrap(),
    )?;
    let mut output = Vec::new();
    loop {
        if engine.step(8)? == Progress::Complete {
            return Ok(output);
        }
        engine.collect();
        if let Some(request) = engine.take_request() {
            let host::Request::Display(text) = request else {
                panic!("unexpected host effect: {request:?}");
            };
            output.push(text);
            engine.resume(if fail {
                Err(Error::new("IOError", "display destination closed"))
            } else {
                Ok(host::Response::Unit)
            })?;
        }
    }
}

#[test]
fn stream_items_display_under_gc_before_bindings_publish() {
    let mut engine = Engine::standard().unwrap();
    assert_eq!(
        display(
            &mut engine,
            "let saved = 42; range 0 3 |> map (add 1)",
            false
        )
        .unwrap(),
        ["1", "2", "3"]
    );
    assert!(engine.inspect(|value| matches!(value, Value::Unit)));
    display(&mut engine, "saved", false).unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
    assert_eq!(
        display(
            &mut engine,
            "items [(), \"a\\u{1b}[2J\", bytes [255]]",
            false
        )
        .unwrap(),
        ["\"a\\u{1b}[2J\"", "<Bytes: 1 bytes>"]
    );
}

#[test]
fn failed_display_discards_publication_and_preserves_script_boundaries() {
    let mut engine = Engine::standard().unwrap();
    assert_eq!(
        display(&mut engine, "let pending = 1; range 0 2", true)
            .unwrap_err()
            .kind,
        "IOError"
    );
    assert_eq!(
        display(&mut engine, "pending", false).unwrap_err().kind,
        "NameError"
    );
    assert_eq!(
        display(&mut engine, "range 0 2; 42", false)
            .unwrap_err()
            .kind,
        "UnconsumedStream"
    );
    engine
        .begin(&rill_syntax::parse("script", "range 0 2").unwrap())
        .unwrap();
    loop {
        match engine.step(16) {
            Err(error) => {
                assert_eq!(error.kind, "UnconsumedStream");
                break;
            }
            Ok(Progress::Yielded) => {}
            result => panic!("unexpected script result: {result:?}"),
        }
    }
}
