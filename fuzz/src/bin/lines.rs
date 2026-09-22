//! Compare chunked UTF-8 and NUL decoding with independent byte-record models.
use rill_runtime::{Engine, Progress, value::Value};
use std::fmt::Write;

fn expected(data: &[u8], limit: usize, binary: bool) -> Result<Vec<Vec<u8>>, &'static str> {
    let delimiter = if binary { 0 } else { b'\n' };
    data.split_inclusive(|byte| *byte == delimiter)
        .map(|record| {
            let terminated = record.last() == Some(&delimiter);
            let line = if terminated {
                &record[..record.len() - 1]
            } else {
                record
            };
            if line.len() > limit {
                return Err("LimitExceeded");
            }
            let line = if terminated && !binary {
                line.strip_suffix(b"\r").unwrap_or(line)
            } else {
                line
            };
            if !binary {
                std::str::from_utf8(line).map_err(|_| "DecodeError")?;
            }
            Ok(line.to_vec())
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
        let binary = input[0] & 128 != 0;
        let expected = expected(data, limit, binary);
        let (decoder, option) = if binary {
            ("text.split_nul_with", "max_record_bytes")
        } else {
            ("text.lines_with", "max_line_bytes")
        };
        let mut source = String::from("items [bytes [],");
        for chunk in data.chunks(chunk_size) {
            source.push_str("bytes [");
            for byte in chunk {
                write!(source, "{byte},").unwrap();
            }
            source.push_str("],");
        }
        write!(source, "] |> {decoder} {{{option}: {limit}}} |> collect").unwrap();
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
                            match value {
                                Value::String(line) if !binary => assert_eq!(line.as_bytes(), text),
                                Value::Bytes(bytes) if binary => assert_eq!(&bytes.0[..], text),
                                _ => panic!("wrong decoded value kind"),
                            }
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
