//! Lexing, grammar construction and completion over the same realistic library source.
use criterion::{Criterion, criterion_group, criterion_main};
use rill_syntax::{parse, token::lex};
use std::hint::black_box;

fn syntax(c: &mut Criterion) {
    let source = include_str!("../../rill-runtime/stdlib/seq.rill");
    assert!(parse("seq", source).is_ok());
    c.bench_function("lex/sequence-library", |b| {
        b.iter(|| lex(black_box(source)).unwrap());
    });
    let entry = format!("{source}\nmodule.record.fi");
    c.bench_function("completion/multiline-context", |b| {
        b.iter(|| rill_syntax::completion::query(black_box(&entry), entry.len()).unwrap());
    });
    for (name, source) in [
        ("complete", "do { let local = 1; local }"),
        ("unfinished", "^echo $(do { let local = 1; loc"),
    ] {
        let cursor = source.rfind("loc").unwrap() + 3;
        let query = rill_syntax::completion::query(source, cursor).unwrap();
        assert_eq!(query.local_names(source), ["local"]);
        c.bench_function(&format!("completion/lexical-scope/{name}"), |b| {
            b.iter(|| query.local_names(black_box(source)));
        });
    }
    c.bench_function("parse/sequence-library", |b| {
        b.iter(|| parse("seq", black_box(source)).unwrap());
    });
}
criterion_group!(benches, syntax);
criterion_main!(benches);
