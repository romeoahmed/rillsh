//! Compare chunked UTF-8 line decoding with a byte-record model under collection.
use rill_runtime::{Engine, Progress, value::Value};
use std::fmt::Write;

fn expected(data: &[u8], limit: usize) -> Result<Vec<String>, &'static str> {
    data.split_inclusive(|byte| *byte == b'\n')
        .map(|record| {
            let terminated = record.ends_with(b"\n");
            let line = if terminated {
                &record[..record.len() - 1]
            } else {
                record
            };
            if line.len() > limit {
                return Err("LimitExceeded");
            }
            let line = if terminated {
                line.strip_suffix(b"\r").unwrap_or(line)
            } else {
                line
            };
            String::from_utf8(line.to_vec()).map_err(|_| "DecodeError")
        })
        .collect()
}

fn main() {
    afl::fuzz!(|input: &[u8]| {
        if input.len() < 2 || input.len() > 4096 {
            return;
        }
        let chunk_size = usize::from(input[0] % 17) + 1;
        let limit = usize::from(input[1] % 65);
        let data = &input[2..];
        let expected = expected(data, limit);
        let mut source = String::from("items [bytes [],");
        for chunk in data.chunks(chunk_size) {
            source.push_str("bytes [");
            for byte in chunk {
                write!(source, "{byte},").unwrap();
            }
            source.push_str("],");
        }
        write!(
            source,
            "] |> lines_with {{max_line_bytes: {limit}}} |> collect"
        )
        .unwrap();
        let module = rill_syntax::parse("lines-fuzz", &source).unwrap();
        let mut engine = Engine::standard().unwrap();
        engine.begin(&module).unwrap();
        loop {
            match engine.step(64) {
                Ok(Progress::Complete) => {
                    let expected = expected.unwrap();
                    engine.inspect(|value| {
                        let Value::List(lines) = value else {
                            panic!("decoder returned a non-list")
                        };
                        assert_eq!(lines.as_slice().len(), expected.len());
                        for (value, text) in lines.as_slice().iter().zip(expected) {
                            assert!(matches!(value, Value::String(line) if line.as_str() == text));
                        }
                    });
                    return;
                }
                Ok(Progress::Yielded) => engine.collect(),
                Ok(Progress::Waiting) => panic!("line decoder requested a host effect"),
                Err(error) => {
                    assert_eq!(error.kind, expected.unwrap_err());
                    return;
                }
            }
        }
    });
}
