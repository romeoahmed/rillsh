//! Explicit text units and byte-preserving lexical paths; no locale-dependent conversion.
use crate::{Error, value::Value};
use gc_arena::Mutation;
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

pub fn display_path<'gc>(mc: &Mutation<'gc>, value: Value<'gc>) -> Result<Value<'gc>, Error> {
    use std::fmt::Write;
    let Value::Path(path) = value else {
        return Err(Error::type_error("display_path requires Path"));
    };
    let mut display = String::new();
    for &byte in path.0.as_os_str().as_bytes() {
        if (32..127).contains(&byte) {
            display.push(char::from(byte));
        } else {
            let _ = write!(display, "\\x{byte:02x}");
        }
    }
    Ok(Value::string(mc, display))
}

pub(super) fn convert<'gc>(mc: &Mutation<'gc>, value: Value<'gc>) -> Result<Value<'gc>, Error> {
    let text = match value {
        Value::String(_) => return Ok(value),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        _ => {
            return Err(Error::type_error(
                "text requires String, Bool, Int, or Float",
            ));
        }
    };
    Ok(Value::string(mc, text))
}

pub(super) fn apply<'gc>(
    mc: &Mutation<'gc>,
    name: &str,
    arguments: &[Value<'gc>],
) -> Result<Value<'gc>, Error> {
    match (name, arguments) {
        ("byte_length", [Value::String(text)]) => super::pure::count(text.len()),
        ("byte_length", [Value::Bytes(bytes)]) => super::pure::count(bytes.0.len()),
        ("byte_length", [Value::Path(path)]) => super::pure::count(path.0.as_os_str().len()),
        ("encode_utf8", [Value::String(text)]) => Ok(Value::bytes(mc, text.as_bytes().to_vec())),
        ("decode_utf8", [Value::Bytes(bytes)]) => {
            let text = std::str::from_utf8(&bytes.0)
                .map_err(|error| Error::new("DecodeError", error.to_string()))?;
            Ok(Value::string(mc, text))
        }
        ("starts_with", [Value::String(prefix), Value::String(text)]) => {
            Ok(Value::Bool(text.starts_with(prefix.as_str())))
        }
        ("ends_with", [Value::String(suffix), Value::String(text)]) => {
            Ok(Value::Bool(text.ends_with(suffix.as_str())))
        }
        ("trim", [Value::String(text)]) => Ok(Value::string(
            mc,
            text.trim_matches([' ', '\t', '\n', '\r', '\u{000c}', '\u{000b}']),
        )),
        ("parse_int" | "parse_float", [Value::String(text)]) => parse_number(name, text),
        ("join_path", [base, child]) => {
            let base = native_path(*base)?;
            let child = native_path(*child)?;
            Value::path(
                mc,
                if child.as_os_str().is_empty() {
                    base.to_path_buf()
                } else {
                    base.join(child)
                },
            )
        }
        ("basename" | "dirname", [value]) => {
            let bytes = native_path(*value)?.as_os_str().as_bytes();
            let result = if bytes.is_empty() {
                b".".as_slice()
            } else {
                let end = bytes
                    .iter()
                    .rposition(|byte| *byte != b'/')
                    .map_or(1, |index| index + 1);
                let trimmed = &bytes[..end];
                let start = trimmed
                    .iter()
                    .rposition(|byte| *byte == b'/')
                    .map_or(0, |index| index + 1);
                if name == "basename" {
                    if start == end {
                        b"/"
                    } else {
                        &trimmed[start..]
                    }
                } else if start == 0 {
                    b"."
                } else {
                    let end = bytes[..start]
                        .iter()
                        .rposition(|byte| *byte != b'/')
                        .map_or(1, |index| index + 1);
                    &bytes[..end]
                }
            };
            Value::path(mc, Path::new(OsStr::from_bytes(result)).to_path_buf())
        }
        _ => Err(Error::type_error(format!("invalid arguments to '{name}'"))),
    }
}

pub fn native_path(value: Value<'_>) -> Result<&Path, Error> {
    let path = match value {
        Value::String(value) => Path::new(gc_arena::Gc::as_ref(value)),
        Value::Bytes(value) => Path::new(OsStr::from_bytes(&gc_arena::Gc::as_ref(value).0)),
        Value::Path(value) => &gc_arena::Gc::as_ref(value).0,
        _ => return Err(Error::type_error("path requires String, Bytes, or Path")),
    };
    if path.as_os_str().as_bytes().contains(&0) {
        return Err(Error::new("TypeError", "path cannot contain NUL"));
    }
    Ok(path)
}

fn parse_number<'gc>(name: &str, text: &str) -> Result<Value<'gc>, Error> {
    let raw = serde_json::from_str::<&serde_json::value::RawValue>(text)
        .map_err(|_| Error::new("DecodeError", "expected one JSON decimal number"))?;
    if raw.get() != text
        || !text.starts_with(|c: char| c == '-' || c.is_ascii_digit())
        || (name == "parse_int" && text.contains(['.', 'e', 'E']))
    {
        return Err(Error::new(
            "DecodeError",
            "expected one JSON decimal number",
        ));
    }
    if name == "parse_int" {
        text.parse()
            .map(Value::Int)
            .map_err(|_| Error::arithmetic())
    } else {
        text.parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map(Value::Float)
            .ok_or_else(Error::arithmetic)
    }
}
