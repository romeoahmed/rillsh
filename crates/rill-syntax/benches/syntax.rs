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
    c.bench_function("parse/sequence-library", |b| {
        b.iter(|| parse("seq", black_box(source)).unwrap());
    });
}
criterion_group!(benches, syntax);
criterion_main!(benches);
