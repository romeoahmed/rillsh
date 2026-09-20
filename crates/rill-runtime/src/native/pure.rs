//! Checked collection and numeric primitives used by the bundled Rill modules.
use super::text;
use crate::{
    Error,
    operations::list,
    value::{List, Value},
};
use gc_arena::Mutation;
use num_traits::ToPrimitive;

pub fn apply<'gc>(mc: &Mutation<'gc>, argument: Value<'gc>) -> Result<Value<'gc>, Error> {
    let request = items(argument)?;
    let [Value::String(name), arguments @ ..] = request.as_slice() else {
        return Err(Error::type_error(
            "primitive request requires an operation name",
        ));
    };
    match (name.as_str(), arguments) {
        ("div", [Value::Int(a), Value::Int(b)]) => a
            .checked_div(*b)
            .map(Value::Int)
            .ok_or_else(Error::arithmetic),
        ("rem", [Value::Int(a), Value::Int(b)]) => a
            .checked_rem(*b)
            .map(Value::Int)
            .ok_or_else(Error::arithmetic),
        ("int", [value @ Value::Int(_)])
        | ("float", [value @ Value::Float(_)])
        | ("number", [value @ (Value::Int(_) | Value::Float(_))]) => Ok(*value),
        ("int", [Value::Float(n)]) => n.to_i64().map(Value::Int).ok_or_else(Error::arithmetic),
        ("float", [Value::Int(n)]) => n.to_f64().map(Value::Float).ok_or_else(Error::arithmetic),
        ("range_start", [Value::Int(start), Value::Int(_)]) => Ok(Value::Int(*start)),
        ("range_start", _) => Err(Error::type_error("range requires Int bounds")),
        ("number", _) => Err(Error::type_error("sum requires Int or Float items")),
        ("is_stream", [value]) => Ok(Value::Bool(matches!(value, Value::Stream(_)))),
        ("text", [value]) => text::convert(mc, *value),
        ("length", [Value::List(values)]) => count(values.as_slice().len()),
        ("length", [Value::Record(fields)]) => count(fields.len()),
        ("concat", [Value::Bytes(a), Value::Bytes(b)]) => {
            Ok(Value::bytes(mc, [a.0.as_ref(), b.0.as_ref()].concat()))
        }
        ("drop", [Value::Int(n), Value::List(values)]) => {
            let n = usize::try_from(*n)
                .map_err(|_| Error::type_error("count must be nonnegative"))?
                .min(values.as_slice().len());
            Ok(Value::List(values.suffix(n).expect("bounded suffix")))
        }
        ("extend", [Value::Record(additions), Value::Record(base)]) => {
            let mut fields = (**additions).clone();
            for (key, value) in base.iter() {
                if fields.insert(key.clone(), *value).is_some() {
                    return Err(Error::new(
                        "TypeError",
                        format!("field '{key}' already exists"),
                    ));
                }
            }
            Ok(Value::Record(crate::heap::record(mc, fields)))
        }
        ("to_record", [Value::Adt(value)]) => Ok(Value::Record(value.fields)),
        ("lookup", [Value::String(key), Value::Record(fields)]) => Ok(list(
            mc,
            fields.get(key.as_str()).copied().into_iter().collect(),
        )),
        ("sort_input" | "sort_key" | "sort", _) => super::sort::apply(mc, name, arguments),
        _ => text::apply(mc, name, arguments),
    }
}

pub(super) fn count<'gc>(count: usize) -> Result<Value<'gc>, Error> {
    i64::try_from(count)
        .map(Value::Int)
        .map_err(|_| Error::arithmetic())
}
pub fn items(value: Value<'_>) -> Result<List<'_>, Error> {
    if let Value::List(items) = value {
        Ok(items)
    } else {
        Err(Error::type_error("expected List"))
    }
}
