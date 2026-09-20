//! End-to-end shell workloads include startup, transport, collection and child cleanup.
use criterion::{Criterion, criterion_group, criterion_main};
use std::{path::Path, process::Command};

fn evaluate(directory: &Path, source: &str, expected: &[u8]) {
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .current_dir(directory)
        .args(["--color=never", "-c", source])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
}

fn pipelines(c: &mut Criterion) {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..128 {
        std::fs::write(directory.path().join(format!("entry-{index}")), b"rill\n").unwrap();
    }
    let mut group = c.benchmark_group("shell");
    for (name, source, expected) in [
        (
            "text",
            r#"range 0 1000 |> map { n => encode_utf8 (text n + "\n") } |> lines |> map parse_int |> filter { n => rem n 2 == 0 } |> sum |> text |> print"#,
            b"249500\n".as_slice(),
        ),
        (
            "filesystem",
            r#"files "." |> filter { entry => entry.kind == "file" } |> map { entry => entry.size } |> sum |> text |> print"#,
            b"640\n",
        ),
        (
            "process",
            r#"range 0 1000 |> map { n => encode_utf8 (text n + "\n") } |> through (job { ^cat }) |> lines |> map parse_int |> filter { n => rem n 2 == 0 } |> sum |> text |> print"#,
            b"249500\n",
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| evaluate(directory.path(), source, expected));
        });
    }
    group.finish();
}
criterion_group!(benches, pipelines);
criterion_main!(benches);
