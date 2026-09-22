//! Prepared evaluation excludes parsing and engine setup; source-to-result includes both.
//! Constructor workloads isolate shape validation and ordered payload construction.
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rill_runtime::{Engine, Progress, value::Value};
use rill_syntax::parse;

const SOURCE: &str = r"
let count = rec { loop n total =>
  if n == 0 then total else loop (n - 1) (total + 1)
}
count 1000 0
";

fn finish(engine: &mut Engine) {
    loop {
        match engine.step(1024).unwrap() {
            Progress::Complete => break,
            Progress::Yielded => {}
            Progress::Waiting => panic!("pure benchmark requested host I/O"),
        }
    }
}

fn evaluation(c: &mut Criterion) {
    let module = parse("count", SOURCE).unwrap();
    c.bench_function("evaluate/tail-calls", |b| {
        b.iter_batched_ref(
            || {
                let mut engine = Engine::default();
                engine.begin(&module).unwrap();
                engine
            },
            |engine| {
                finish(engine);
                assert!(engine.inspect(|v| matches!(v, Value::Int(1000))));
            },
            BatchSize::SmallInput,
        );
    });
    c.bench_function("continuation/switch", |b| {
        b.iter_batched_ref(
            || {
                let mut engine = Engine::default();
                engine.begin(&module).unwrap();
                engine.suspend(1).unwrap();
                engine.begin(&module).unwrap();
                engine
            },
            |engine| {
                engine.suspend(2).unwrap();
                engine.activate(1).unwrap();
                engine.suspend(1).unwrap();
                engine.activate(2).unwrap();
            },
            BatchSize::SmallInput,
        );
    });
    c.bench_function("source-to-result/tail-calls", |b| {
        b.iter(|| {
            let module = parse("count", SOURCE).unwrap();
            let mut engine = Engine::default();
            engine.begin(&module).unwrap();
            finish(&mut engine);
            assert!(engine.inspect(|v| matches!(v, Value::Int(1000))));
        });
    });
    let mut metadata = Engine::standard().unwrap();
    let fields = (0..10_000)
        .map(|index| format!("field_{index:05}: {index}"))
        .collect::<Vec<_>>()
        .join(", ");
    metadata
        .begin(&parse("metadata", &format!("let namespace = {{{fields}}}")).unwrap())
        .unwrap();
    finish(&mut metadata);
    for (name, input) in [
        ("completion/published", "ma"),
        ("completion/wide-record", "namespace.field_09"),
    ] {
        let query = rill_syntax::completion::query(input, input.len()).unwrap();
        assert!(!metadata.complete(&query).is_empty());
        c.bench_function(name, |b| {
            b.iter(|| metadata.complete(std::hint::black_box(&query)));
        });
    }
    let mut session = Engine::standard().unwrap();
    let publication = parse(
        "publication",
        "let saved = do { let data = [40, 2]; { a b => a + b + data[0] } 0 }; ()",
    )
    .unwrap();
    c.bench_function("session/replace-partial", |b| {
        b.iter(|| {
            session.begin(&publication).unwrap();
            finish(&mut session);
        });
    });
    pipelines(c);
    json(c);
    constructors(c);
    record_patterns(c);
    allocation(c);
}

fn json(c: &mut Criterion) {
    let fixture = parse(
        "json-data",
        r#"let rows = range 0 1000 |> map { n => {name: "sample", data: [n, true, null]} } |> collect"#,
    ).unwrap();
    let encode = parse("json-encode", "to_json rows").unwrap();
    c.bench_function("json/encode-records", |b| {
        b.iter_batched_ref(
            || {
                let mut engine = Engine::standard().unwrap();
                engine.begin(&fixture).unwrap();
                finish(&mut engine);
                engine.begin(&encode).unwrap();
                engine
            },
            |engine| {
                finish(engine);
                assert!(engine.inspect(|value| matches!(value, Value::Bytes(bytes)
                    if bytes.0.starts_with(br#"[{"name":"sample","data":[0,true,null]}"#)
                        && bytes.0.ends_with(br#"{"name":"sample","data":[999,true,null]}]"#))));
            },
            BatchSize::LargeInput,
        );
    });
}

fn allocation(c: &mut Criterion) {
    for (name, source, expected) in [
        (
            "gc/discarded-cycles",
            r"let repeat = rec { loop n => if n == 0 then 0 else do {
  let discarded = rec { self () => self () }
  loop (n - 1)
} }
repeat 1000"
                .into(),
            0,
        ),
        (
            "gc/scalar-materialization",
            format!("text.scalars '{}' |> length", "x".repeat(10_000)),
            10_000,
        ),
    ] {
        let module = parse(name, &source).unwrap();
        c.bench_function(name, |b| {
            b.iter_batched_ref(
                || {
                    let mut engine = Engine::standard().unwrap();
                    engine.begin(&module).unwrap();
                    engine
                },
                |engine| {
                    finish(engine);
                    assert!(
                        engine.inspect(|value| matches!(value, Value::Int(n) if n == expected))
                    );
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn record_patterns(c: &mut Criterion) {
    for width in [8, 1024] {
        let fields = (0..width)
            .map(|index| format!("field_{index}: {index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let pattern = (0..width / 2)
            .rev()
            .map(|index| format!("field_{index}: _"))
            .chain(std::iter::once("..rest".into()))
            .collect::<Vec<_>>()
            .join(", ");
        let declaration = parse("declaration", &format!("let row = {{{fields}}}")).unwrap();
        let application = parse("matching", &format!("let {{{pattern}}} = row; rest")).unwrap();
        c.bench_function(&format!("match/record-rest/{width}"), |b| {
            b.iter_batched_ref(
                || {
                    let mut engine = Engine::default();
                    engine.begin(&declaration).unwrap();
                    finish(&mut engine);
                    engine.begin(&application).unwrap();
                    engine
                },
                |engine| {
                    finish(engine);
                    assert!(engine.inspect(
                        |value| matches!(value, Value::Record(fields) if fields.len() == width / 2)
                    ));
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn constructors(c: &mut Criterion) {
    for width in [8, 1024] {
        let names = (0..width)
            .map(|index| format!("field_{index}"))
            .collect::<Vec<_>>();
        let fields = names
            .iter()
            .rev()
            .enumerate()
            .map(|(index, name)| format!("{name}: {index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let declaration = parse(
            "declaration",
            &format!(
                "struct Row {{{}}}; let fields = {{{fields}}}",
                names.join(", ")
            ),
        )
        .unwrap();
        let application = parse("construction", "Row fields").unwrap();
        c.bench_function(&format!("construct/fields/{width}"), |b| {
            b.iter_batched_ref(
                || {
                    let mut engine = Engine::default();
                    engine.begin(&declaration).unwrap();
                    finish(&mut engine);
                    engine.begin(&application).unwrap();
                    engine
                },
                |engine| {
                    finish(engine);
                    assert!(engine.inspect(
                        |value| matches!(value, Value::Adt(value) if value.fields.len() == width)
                    ));
                },
                BatchSize::LargeInput,
            );
        });
    }
}

fn pipelines(c: &mut Criterion) {
    for (name, source, expected) in [
        (
            "evaluate/sequential-flat-map",
            "range 0 100 |> flat_map { n => range 0 10 } |> sum",
            4_500,
        ),
        (
            "evaluate/grouping",
            "range 0 1000 |> count_by { n => string (rem n 10) } |> entries |> map { [key, count] => count } |> sum",
            1_000,
        ),
        (
            "evaluate/json-roundtrip",
            "range 0 1000 |> collect |> to_json |> from_json |> sum",
            499_500,
        ),
        (
            "evaluate/stable-sort",
            "range 0 1000 |> collect |> reverse |> sort_by identity |> sum",
            499_500,
        ),
        (
            "evaluate/lazy-map-filter-fold",
            "range 0 1000 |> map (add 1) |> filter { n => rem n 2 == 0 } |> fold add 0",
            250_500,
        ),
        (
            "evaluate/producer-lifecycle",
            r"seq.produce {
  acquire: { () => 0 },
  step: { n => if n < 1000 then some [n, n + 1] else Option.None },
  release: { state reason => () }
} |> sum",
            499_500,
        ),
        (
            "evaluate/merge-transforms",
            "merge [range 0 1000 |> map (add 1), range 0 1000 |> filter { n => rem n 2 == 0 }] |> sum",
            750_000,
        ),
    ] {
        let module = parse(name, source).unwrap();
        c.bench_function(name, |b| {
            b.iter_batched_ref(
                || {
                    let mut engine = Engine::standard().unwrap();
                    engine.begin(&module).unwrap();
                    engine
                },
                |engine| {
                    finish(engine);
                    assert!(
                        engine.inspect(
                            |value| matches!(value, Value::Int(value) if value == expected)
                        )
                    );
                },
                BatchSize::SmallInput,
            );
        });
    }
}
criterion_group!(benches, evaluation);
criterion_main!(benches);
