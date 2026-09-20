//! Atomic structural matching into a temporary binding set.
use crate::{Error, operations, value::Value};
use gc_arena::{Gc, Mutation};
use rill_syntax::ast::{Pattern, Rest};

type Bindings<'pattern, 'gc> = Vec<(&'pattern str, Value<'gc>)>;

pub fn bind<'pattern, 'gc>(
    mc: &Mutation<'gc>,
    pattern: &'pattern Pattern,
    value: Value<'gc>,
    lookup: &impl Fn(&str) -> Result<Value<'gc>, Error>,
) -> Result<Bindings<'pattern, 'gc>, Error> {
    attempt(mc, pattern, value, lookup)?
        .ok_or_else(|| Error::new("MatchError", "value does not match the binding pattern"))
}

pub fn attempt<'pattern, 'gc>(
    mc: &Mutation<'gc>,
    pattern: &'pattern Pattern,
    value: Value<'gc>,
    lookup: &impl Fn(&str) -> Result<Value<'gc>, Error>,
) -> Result<Option<Bindings<'pattern, 'gc>>, Error> {
    let mut bindings = Vec::new();
    Ok(matches(mc, pattern, value, &mut bindings, lookup)?.then_some(bindings))
}

fn matches<'pattern, 'gc>(
    mc: &Mutation<'gc>,
    pattern: &'pattern Pattern,
    value: Value<'gc>,
    bindings: &mut Bindings<'pattern, 'gc>,
    lookup: &impl Fn(&str) -> Result<Value<'gc>, Error>,
) -> Result<bool, Error> {
    Ok(match (pattern, value) {
        (Pattern::Ignore, _) => true,
        (Pattern::Bind(name), value) => {
            bindings.push((name, value));
            true
        }
        (Pattern::Literal(literal), value) => {
            let expected = operations::literal(mc, literal)?;
            expected.kind() == value.kind() && expected.equals(value)?
        }
        (Pattern::List(items, rest), Value::List(values)) => {
            if values.as_slice().len() < items.len()
                || (rest.is_none() && values.as_slice().len() != items.len())
            {
                return Ok(false);
            }
            for (pattern, value) in items.iter().zip(values.as_slice()) {
                if !matches(mc, pattern, *value, bindings, lookup)? {
                    return Ok(false);
                }
            }
            if let Some(name) = rest
                && name != "_"
            {
                bindings.push((
                    name,
                    Value::List(values.suffix(items.len()).expect("length checked above")),
                ));
            }
            true
        }
        (Pattern::Record(patterns, rest), Value::Record(fields)) => {
            if matches!(rest, Rest::Exact) && fields.len() != patterns.len() {
                return Ok(false);
            }
            for (key, pattern) in patterns {
                let Some(value) = fields.get(key) else {
                    return Ok(false);
                };
                if !matches(mc, pattern, *value, bindings, lookup)? {
                    return Ok(false);
                }
            }
            if let Rest::Bind(name) = rest
                && name != "_"
            {
                bindings.push((name, record_rest(mc, &fields, patterns)));
            }
            true
        }
        (Pattern::Constructor(path, payload), value) => {
            let mut descriptor = lookup(&path[0])?;
            for name in &path[1..] {
                descriptor = descriptor.field(name)?;
            }
            let descriptor = match descriptor {
                Value::Constructor(descriptor) => descriptor,
                Value::Adt(value) if value.descriptor.fields.is_empty() => value.descriptor,
                _ => {
                    return Err(Error::type_error(
                        "pattern path does not name a constructor",
                    ));
                }
            };
            if let Some(payload) = payload
                && let Pattern::Record(fields, _) = &**payload
            {
                for (key, _) in fields {
                    if !descriptor.fields.contains(key) {
                        return Err(Error::type_error(format!(
                            "constructor has no field '{key}'"
                        )));
                    }
                }
            }
            let Value::Adt(adt) = value else {
                return Ok(false);
            };
            if !Gc::ptr_eq(descriptor, adt.descriptor) {
                return Ok(false);
            }
            if let Some(pattern) = payload {
                matches(mc, pattern, Value::Record(adt.fields), bindings, lookup)?
            } else {
                descriptor.fields.is_empty()
            }
        }
        _ => false,
    })
}

/// Reuse the record's key index, then copy unmatched fields in insertion order.
fn record_rest<'gc>(
    mc: &Mutation<'gc>,
    fields: &crate::value::Record<'gc>,
    patterns: &[(String, Pattern)],
) -> Value<'gc> {
    let mut matched = vec![false; fields.len()];
    for (key, _) in patterns {
        matched[fields.get_index_of(key).expect("matched record field")] = true;
    }
    Value::Record(crate::heap::record(
        mc,
        fields
            .iter()
            .zip(matched)
            .filter(|(_, matched)| !matched)
            .map(|((key, value), _)| (key.clone(), *value))
            .collect(),
    ))
}
