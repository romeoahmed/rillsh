//! Exercise codecs through ordinary language calls and collect between VM quanta.
use rill_runtime::{Engine, Progress, value::Value};
use std::fmt::Write;
fn main() {
    afl::fuzz!(|data: &[u8]| {
        if data.len() > 8192 {
            return;
        }
        let mut source = String::from("let value = from_json (bytes [");
        for byte in data {
            write!(source, "{byte},").unwrap();
        }
        source.push_str("]); from_json (to_json value) == value");
        let module = rill_syntax::parse("json-fuzz", &source).unwrap();
        let mut engine = Engine::standard().unwrap();
        engine.begin(&module).unwrap();
        loop {
            match engine.step(128) {
                Ok(Progress::Complete) => {
                    assert!(engine.inspect(|value| matches!(value, Value::Bool(true))));
                    break;
                }
                Ok(Progress::Yielded) => engine.collect(),
                Ok(Progress::Waiting) => panic!("codec requested a host effect"),
                Err(error) => {
                    assert!(matches!(
                        error.kind.as_str(),
                        "DecodeError" | "ArithmeticError" | "LimitExceeded"
                    ));
                    break;
                }
            }
        }
    });
}
