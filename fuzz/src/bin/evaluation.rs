//! A bounded pure domain checks curried recursion, parked continuations and GC against integer arithmetic.
use rill_runtime::{Engine, Progress, value::Value};
use std::fmt::Write;
fn main() {
    afl::fuzz!(|data: &[u8]| {
        let mut source = String::from(
            "let fold = rec { loop total xs => match xs of { [] => total, [x, ..tail] => loop (total + x * x) tail } }; fold 0 [",
        );
        let mut expected = 0_i64;
        for pair in data.as_chunks::<2>().0.iter().take(64) {
            let value = i64::from(i16::from_le_bytes(*pair));
            expected += value * value;
            write!(source, "{value},").unwrap();
        }
        source.push(']');
        let module = rill_syntax::parse("evaluation-fuzz", &source).unwrap();
        let mut engine = Engine::default();
        engine.begin(&module).unwrap();
        let interleaved = rill_syntax::parse("interleaved", "let unrelated = [1, 2, 3]").unwrap();
        let mut switches = data.iter().take(8);
        loop {
            match engine.step(16).unwrap() {
                Progress::Complete => {
                    assert!(
                        engine.inspect(|value| matches!(value, Value::Int(n) if n == expected))
                    );
                    return;
                }
                Progress::Yielded => {
                    if switches.next().is_some_and(|byte| byte & 1 != 0) {
                        engine.suspend(1).unwrap();
                        engine.collect();
                        engine.begin(&interleaved).unwrap();
                        loop {
                            match engine.step(16).unwrap() {
                                Progress::Complete => break,
                                Progress::Yielded => engine.collect(),
                                Progress::Waiting => {
                                    panic!("unrelated entry requested a host effect")
                                }
                            }
                        }
                        engine.activate(1).unwrap();
                    }
                    engine.collect();
                }
                Progress::Waiting => panic!("pure evaluation requested a host effect"),
            }
        }
    });
}
