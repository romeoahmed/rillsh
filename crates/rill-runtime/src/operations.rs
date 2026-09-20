//! Checked scalar operations; Rill never uses Rust's implicit numeric coercions.
use crate::{
    Error,
    value::{List, Value},
};
use gc_arena::{Gc, Mutation};
use rill_syntax::ast::{Binary, Literal, Unary};

pub fn literal<'gc>(mc: &Mutation<'gc>, literal: &Literal) -> Result<Value<'gc>, Error> {
    Ok(match literal {
        Literal::Unit => Value::Unit,
        Literal::Null => Value::Null,
        Literal::Bool(b) => Value::Bool(*b),
        Literal::String(s) => Value::string(mc, s.clone()),
        Literal::Integer(n) => Value::Int(
            n.replace('_', "")
                .parse()
                .map_err(|_| Error::arithmetic())?,
        ),
        Literal::Float(n) => finite(
            n.replace('_', "")
                .parse()
                .map_err(|_| Error::arithmetic())?,
        )?,
    })
}
fn finite<'gc>(value: f64) -> Result<Value<'gc>, Error> {
    if value.is_finite() {
        Ok(Value::Float(value))
    } else {
        Err(Error::arithmetic())
    }
}

pub fn unary(op: Unary, value: Value<'_>) -> Result<Value<'_>, Error> {
    match (op, value) {
        (Unary::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (Unary::Negate, Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(Error::arithmetic),
        (Unary::Negate, Value::Float(n)) => Ok(Value::Float(-n)),
        (Unary::Not, _) => Err(Error::type_error("not requires Bool")),
        (Unary::Negate, _) => Err(Error::type_error("negation requires Int or Float")),
    }
}

pub fn binary<'gc>(
    mc: &Mutation<'gc>,
    op: Binary,
    left: Value<'gc>,
    right: Value<'gc>,
) -> Result<Value<'gc>, Error> {
    match (op, left, right) {
        (Binary::Add, Value::Int(a), Value::Int(b)) => a
            .checked_add(b)
            .map(Value::Int)
            .ok_or_else(Error::arithmetic),
        (Binary::Subtract, Value::Int(a), Value::Int(b)) => a
            .checked_sub(b)
            .map(Value::Int)
            .ok_or_else(Error::arithmetic),
        (Binary::Multiply, Value::Int(a), Value::Int(b)) => a
            .checked_mul(b)
            .map(Value::Int)
            .ok_or_else(Error::arithmetic),
        (Binary::Add, Value::Float(a), Value::Float(b)) => finite(a + b),
        (Binary::Subtract, Value::Float(a), Value::Float(b)) => finite(a - b),
        (Binary::Multiply, Value::Float(a), Value::Float(b)) => finite(a * b),
        (Binary::Divide, Value::Float(a), Value::Float(b)) => finite(a / b),
        (Binary::Add, Value::String(a), Value::String(b)) => {
            Ok(Value::string(mc, format!("{a}{b}")))
        }
        (Binary::Less | Binary::LessEqual | Binary::Greater | Binary::GreaterEqual, a, b) => {
            let order = match (a, b) {
                (Value::Int(a), Value::Int(b)) => Some(a.cmp(&b)),
                (Value::Float(a), Value::Float(b)) => a.partial_cmp(&b),
                (Value::String(a), Value::String(b)) => Some(a.cmp(&b)),
                _ => None,
            }
            .ok_or_else(|| {
                Error::type_error("ordering requires two Strings or numbers of the same kind")
            })?;
            Ok(Value::Bool(match op {
                Binary::Less => order.is_lt(),
                Binary::LessEqual => order.is_le(),
                Binary::Greater => order.is_gt(),
                _ => order.is_ge(),
            }))
        }
        (Binary::With, base, Value::Record(replacements)) => {
            let fields = match base {
                Value::Record(fields) => fields,
                Value::Adt(adt) => adt.fields,
                _ => {
                    return Err(Error::type_error(
                        "update requires a Record or nominal value",
                    ));
                }
            };
            let mut fields = (*fields).clone();
            for (key, replacement) in replacements.iter() {
                let value = fields.get_mut(key).ok_or_else(|| {
                    Error::new("MissingField", format!("record has no field '{key}'"))
                })?;
                *value = *replacement;
            }
            let fields = crate::heap::record(mc, fields);
            Ok(if let Value::Adt(adt) = base {
                Value::Adt(Gc::new(
                    mc,
                    crate::value::Adt {
                        descriptor: adt.descriptor,
                        fields,
                    },
                ))
            } else {
                Value::Record(fields)
            })
        }
        _ => Err(Error::type_error(format!(
            "invalid {} and {} operands",
            left.kind(),
            right.kind()
        ))),
    }
}

pub fn index<'gc>(value: Value<'gc>, index: Value<'gc>) -> Result<Value<'gc>, Error> {
    match (value, index) {
        (Value::List(list), Value::Int(index)) => usize::try_from(index)
            .ok()
            .and_then(|i| list.as_slice().get(i))
            .copied()
            .ok_or_else(|| Error::new("IndexError", "list index is out of bounds")),
        (Value::Record(_), Value::String(key)) => value.field(&key),
        _ => Err(Error::type_error(
            "indexing requires a List and Int, or Record and String",
        )),
    }
}

pub fn list<'gc>(mc: &Mutation<'gc>, values: Vec<Value<'gc>>) -> Value<'gc> {
    Value::List(List::new(mc, values))
}
