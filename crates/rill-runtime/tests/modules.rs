//! Module identity, exports and initialization across owned filesystem requests.
use rill_runtime::{Engine, Error, Progress, value::Value};
use std::{fs, path::Path};

fn service(engine: &mut Engine) -> Result<(), Error> {
    use rill_runtime::host::{Request, Response};
    if let Some(request) = engine.take_request() {
        let response = (|| -> std::io::Result<Response> {
            match request {
                Request::OpenModule { directory, path } => {
                    directory.source(&path).map(Response::ModuleFile)
                }
                Request::ReadModule(mut file) => {
                    let text = file.read(16 * 1024 * 1024)?;
                    Ok(Response::ModuleText { file, text })
                }
                _ => panic!("unexpected module fixture request"),
            }
        })()
        .map_err(Error::from);
        engine.resume(response)?;
    }
    Ok(())
}

fn run(engine: &mut Engine, directory: &Path, source: &str) -> Result<(), Error> {
    engine.begin_in(&rill_syntax::parse("entry", source).unwrap(), directory)?;
    loop {
        if engine.step(16)? == Progress::Complete {
            return Ok(());
        }
        engine.collect();
        service(engine)?;
    }
}

#[test]
fn exports_preserve_lexical_values_and_nominal_identity_across_aliases() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("point.rill"), "struct Point {x}; let private = 40; fn answer value = value + private; export {Point, answer}; let later = 0").unwrap();
    fs::hard_link(
        directory.path().join("point.rill"),
        directory.path().join("alias.rill"),
    )
    .unwrap();
    let mut engine = Engine::default();
    run(&mut engine, directory.path(), "import \"point.rill\" as p; import \"alias.rill\" as a; match p.Point {x: 2} of { a.Point {x} => a.answer x }").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
    assert_eq!(
        run(&mut engine, directory.path(), "p.private")
            .unwrap_err()
            .kind,
        "MissingField"
    );
    fs::write(directory.path().join("point.rill"), "missing").unwrap();
    let query = rill_syntax::completion::query("p.", 2).unwrap();
    let names: Vec<_> = engine
        .complete(&query)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["Point", "answer"]);

    run(
        &mut engine,
        directory.path(),
        "import \"alias.rill\" as saved; saved.answer 2",
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
}

#[test]
fn failed_modules_can_be_retried_and_do_not_inherit_session_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("module.rill");
    fs::write(
        &source,
        "struct Point {x}; let value = secret; export {Point, value}",
    )
    .unwrap();
    let mut engine = Engine::default();
    run(&mut engine, directory.path(), "let secret = 42").unwrap();
    assert_eq!(
        run(&mut engine, directory.path(), "import \"module.rill\" as m")
            .unwrap_err()
            .kind,
        "NameError"
    );
    fs::write(
        &source,
        "struct Point {x}; let value = 42; export {Point, value}",
    )
    .unwrap();
    run(
        &mut engine,
        directory.path(),
        "import \"module.rill\" as m; m.value",
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
}

#[test]
fn cycles_are_detected_by_file_identity_and_do_not_poison_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("a.rill"),
        "import \"b.rill\" as b; export {b}",
    )
    .unwrap();
    fs::write(
        directory.path().join("b.rill"),
        "import \"alias.rill\" as a",
    )
    .unwrap();
    fs::hard_link(
        directory.path().join("a.rill"),
        directory.path().join("alias.rill"),
    )
    .unwrap();
    let mut engine = Engine::default();
    assert_eq!(
        run(&mut engine, directory.path(), "import \"a.rill\" as a")
            .unwrap_err()
            .kind,
        "ImportCycle"
    );
    fs::write(
        directory.path().join("b.rill"),
        "let answer = 42; export {answer}",
    )
    .unwrap();
    run(
        &mut engine,
        directory.path(),
        "import \"a.rill\" as a; a.b.answer",
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
}

#[test]
fn imports_retain_the_entry_directory_across_rename() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("original");
    fs::create_dir(&original).unwrap();
    fs::write(
        original.join("value.rill"),
        "let value = 42; export {value}",
    )
    .unwrap();
    let mut engine = Engine::default();
    engine
        .begin_in(
            &rill_syntax::parse("entry", "import \"value.rill\" as m; m.value").unwrap(),
            &original,
        )
        .unwrap();
    fs::rename(&original, directory.path().join("moved")).unwrap();
    while engine.step(16).unwrap() != Progress::Complete {
        engine.collect();
        service(&mut engine).unwrap();
    }
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
}

#[test]
fn imports_suspend_before_filesystem_work_and_survive_unrelated_entries() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("value.rill"),
        "let answer = 42; export {answer}",
    )
    .unwrap();
    let mut engine = Engine::default();
    engine
        .begin_in(
            &rill_syntax::parse("entry", "import \"value.rill\" as value; value.answer").unwrap(),
            directory.path(),
        )
        .unwrap();
    assert_eq!(engine.step(128).unwrap(), Progress::Waiting);
    engine.suspend(1).unwrap();
    run(&mut engine, directory.path(), "let other = 7").unwrap();
    engine.collect();
    engine.activate(1).unwrap();
    service(&mut engine).unwrap();
    while engine.step(16).unwrap() != Progress::Complete {
        engine.collect();
        service(&mut engine).unwrap();
    }
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
    run(&mut engine, directory.path(), "other").unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Int(7))));
}

#[test]
fn exports_are_evaluated_at_their_position() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("bad.rill"),
        "export {later}; let later = 42",
    )
    .unwrap();
    fs::write(directory.path().join("empty.rill"), "let private = 42").unwrap();
    let mut engine = Engine::default();
    assert_eq!(
        run(&mut engine, directory.path(), "import \"bad.rill\" as m")
            .unwrap_err()
            .kind,
        "NameError"
    );
    run(
        &mut engine,
        directory.path(),
        "import \"empty.rill\" as m; m == {}",
    )
    .unwrap();
    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
}
#[test]
fn bundled_imports_do_not_require_a_filesystem_base() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let module =
        rill_syntax::parse("bundle-only", "import \"std:core\" as core; core.add 20 22").unwrap();
    let mut engine = Engine::default();
    engine.begin_in(&module, &missing).unwrap();
    loop {
        match engine.step(64).unwrap() {
            Progress::Complete => break,
            Progress::Yielded => engine.collect(),
            Progress::Waiting => panic!("bundled import requested host I/O"),
        }
    }
    assert!(engine.inspect(|value| matches!(value, Value::Int(42))));
}
