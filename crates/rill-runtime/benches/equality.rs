//! Prepared comparison includes VM quanta and GC pacing; graph construction is untimed.
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rill_runtime::{Engine, Progress, value::Value};
use rill_syntax::parse;

fn finish(engine: &mut Engine) {
    loop {
        match engine.step(1024).unwrap() {
            Progress::Complete => break,
            Progress::Yielded => {}
            Progress::Waiting => panic!("pure benchmark requested host I/O"),
        }
    }
}

fn equality(c: &mut Criterion) {
    let wide = (0..4096)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let text = "x".repeat(1024 * 1024);
    let comparison = parse("compare", "left == right").unwrap();
    for (name, setup, expected) in [
        (
            "small-list",
            "let left = [0, 1, 2, 3]; let right = [0, 1, 2, 3]".into(),
            true,
        ),
        (
            "shared-graph",
            r"
fn graph depth = if depth == 0 then {value: 42} else do {
  let child = graph (depth - 1)
  [child, child]
}
let left = graph 40
let right = graph 40
"
            .into(),
            true,
        ),
        (
            "wide-list",
            format!("let left = [{wide}]; let right = [{wide}]"),
            true,
        ),
        (
            "large-text",
            format!("fn make () = {text:?} + \"!\"; let left = make (); let right = make ()"),
            true,
        ),
        (
            "wide-list-first-mismatch",
            format!("let left = [0, {wide}]; let right = [1, {wide}]"),
            false,
        ),
        (
            "wide-list-last-mismatch",
            format!("let left = [{wide}, 0]; let right = [{wide}, 1]"),
            false,
        ),
        (
            "large-text-last-mismatch",
            format!("let left = {text:?} + \"a\"; let right = {text:?} + \"b\""),
            false,
        ),
    ] {
        let setup = parse("setup", &setup).unwrap();
        c.bench_function(&format!("equality/{name}"), |b| {
            b.iter_batched_ref(
                || {
                    let mut engine = Engine::default();
                    engine.begin(&setup).unwrap();
                    finish(&mut engine);
                    engine.begin(&comparison).unwrap();
                    engine
                },
                |engine| {
                    finish(engine);
                    assert!(
                        engine.inspect(
                            |value| matches!(value, Value::Bool(equal) if equal == expected)
                        )
                    );
                },
                BatchSize::LargeInput,
            );
        });
    }
}
criterion_group!(benches, equality);
criterion_main!(benches);
