//! Suspended entries share session identities, never lexical snapshots or mutable stacks.
use rill_runtime::{
    Engine, Error, Progress,
    host::{Request, Response},
    value::Value,
};
use rill_syntax::parse;

fn begin(engine: &mut Engine, source: &str) {
    engine
        .begin(&parse("continuation-test", source).unwrap())
        .unwrap();
}
fn next_request(engine: &mut Engine) -> Result<Option<Request>, Error> {
    loop {
        match engine.step(1)? {
            Progress::Complete => return Ok(None),
            Progress::Yielded => engine.collect(),
            Progress::Waiting => match engine.take_request().unwrap() {
                Request::OpenModule { directory, path } => {
                    engine.resume(Ok(Response::ModuleFile(directory.source(&path).unwrap())))?;
                }
                Request::ReadModule(mut file) => {
                    let text = file.read(16 * 1024 * 1024).unwrap();
                    engine.resume(Ok(Response::ModuleText { file, text }))?;
                }
                request => return Ok(Some(request)),
            },
        }
    }
}
fn finish(engine: &mut Engine) -> Result<(), Error> {
    assert!(next_request(engine)?.is_none());
    Ok(())
}
fn waiting(engine: &mut Engine) {
    assert!(matches!(next_request(engine).unwrap(), Some(Request::Pwd)));
}
fn number(engine: &Engine) -> i64 {
    engine.inspect(|value| match value {
        Value::Int(n) => n,
        _ => panic!("expected Int"),
    })
}
#[test]
fn parked_stacks_survive_collection_and_merge_only_new_bindings() {
    let mut engine = Engine::standard().unwrap();
    begin(&mut engine, "let captured = 40");
    finish(&mut engine).unwrap();
    begin(&mut engine, "let resumed = captured + 2; pwd (); resumed");
    waiting(&mut engine);
    engine.suspend(1).unwrap();
    begin(&mut engine, "let captured = 99; let interim = 7");
    finish(&mut engine).unwrap();
    engine.collect();
    engine.activate(1).unwrap();
    engine.resume(Ok(Response::Path("/".into()))).unwrap();
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 42);
    begin(&mut engine, "captured + interim + resumed");
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 148);
}
#[test]
fn resumption_detects_nominal_conflicts_before_publication() {
    let mut engine = Engine::standard().unwrap();
    begin(
        &mut engine,
        "struct Saved {old}; let unpublished = 1; pwd ()",
    );
    waiting(&mut engine);
    engine.suspend(1).unwrap();
    begin(
        &mut engine,
        "struct Saved {new}; let current = Saved {new: 42}",
    );
    finish(&mut engine).unwrap();
    engine.activate(1).unwrap();
    engine.resume(Ok(Response::Path("/".into()))).unwrap();
    assert_eq!(finish(&mut engine).unwrap_err().kind, "NameError");
    begin(&mut engine, "current.new");
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 42);
    begin(&mut engine, "unpublished");
    assert_eq!(finish(&mut engine).unwrap_err().kind, "NameError");
}
#[test]
fn a_foreground_caller_receives_a_value_without_rebinding_its_environment() {
    let mut engine = Engine::standard().unwrap();
    begin(&mut engine, "let answer = 40");
    finish(&mut engine).unwrap();
    begin(&mut engine, "pwd (); let answer = 99; 2");
    waiting(&mut engine);
    engine.suspend(1).unwrap();
    begin(&mut engine, "let value = pwd (); value + answer");
    waiting(&mut engine);
    engine.suspend(2).unwrap();
    engine.activate(1).unwrap();
    engine.resume(Ok(Response::Path("/".into()))).unwrap();
    finish(&mut engine).unwrap();
    engine.collect();
    engine.return_to(2, Ok(())).unwrap();
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 42);
    begin(&mut engine, "answer");
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 99);
}

#[test]
fn parked_module_initialization_keeps_identity_and_import_context() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("module.rill"),
        "pwd (); struct Item {value}; export {Item}",
    )
    .unwrap();
    let module = parse(
        "entry",
        "import \"module.rill\" as saved; (saved.Item {value: 42}).value",
    )
    .unwrap();
    let mut engine = Engine::standard().unwrap();
    engine.begin_in(&module, directory.path()).unwrap();
    waiting(&mut engine);
    engine.suspend(1).unwrap();
    engine
        .begin_in(
            &parse("other", "import \"module.rill\" as other").unwrap(),
            directory.path(),
        )
        .unwrap();
    assert_eq!(finish(&mut engine).unwrap_err().kind, "ImportBusy");
    engine.collect();
    engine.activate(1).unwrap();
    engine
        .resume(Ok(Response::Path(directory.path().into())))
        .unwrap();
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 42);
    engine.begin_in(&parse("identity", "import \"module.rill\" as other; match other.Item {value: 42} of { saved.Item {value} => value }").unwrap(), directory.path()).unwrap();
    finish(&mut engine).unwrap();
    assert_eq!(number(&engine), 42);
}
