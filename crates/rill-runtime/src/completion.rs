//! Metadata leaves the arena as bounded text, with no retained language values.
use crate::value::Value;
use rill_syntax::completion::Query;
use std::collections::{BTreeMap, HashMap};

pub fn members(globals: &HashMap<String, Value<'_>>, query: &Query) -> Vec<(String, String)> {
    let Some((root, fields)) = query.parents.split_first() else {
        return candidates(globals.iter(), &query.prefix);
    };
    let Some(mut value) = globals.get(root).copied() else {
        return Vec::new();
    };
    for field in fields {
        let Ok(next) = value.field(field) else {
            return Vec::new();
        };
        value = next;
    }
    match value {
        Value::Record(record) => candidates(record.iter(), &query.prefix),
        Value::Adt(adt) => candidates(adt.fields.iter(), &query.prefix),
        _ => Vec::new(),
    }
}
fn candidates<'a, 'gc: 'a>(
    entries: impl Iterator<Item = (&'a String, &'a Value<'gc>)>,
    prefix: &str,
) -> Vec<(String, String)> {
    let mut selected = BTreeMap::new();
    for (name, value) in entries {
        // Arbitrary String keys remain accessible through indexed expressions.
        if name.starts_with(prefix) && identifier(name) {
            if selected.len() == 200
                && selected
                    .last_key_value()
                    .is_some_and(|(last, _)| name >= *last)
            {
                continue;
            }
            selected.insert(name, value);
            if selected.len() > 200 {
                selected.pop_last();
            }
        }
    }
    let mut remaining: usize = 1024 * 1024;
    selected
        .into_iter()
        .take_while(|(name, _)| name.len() <= 1024 * 1024)
        .map(|(name, value)| {
            (
                name,
                value.signature().map_or_else(
                    || value.kind().into(),
                    |parameters| {
                        let mut signature = format!("Function: {name} {parameters}");
                        if let Some(doc) = value.documentation().and_then(|doc| doc.lines().next())
                        {
                            signature.push_str(" — ");
                            signature.push_str(&crate::presentation::preview(format_args!(
                                "{}",
                                doc.escape_debug()
                            )));
                        }
                        signature
                    },
                ),
            )
        })
        .take_while(|(name, description)| {
            let Some(bytes) = remaining.checked_sub(name.len() + description.len()) else {
                return false;
            };
            remaining = bytes;
            true
        })
        .map(|(name, description)| (name.clone(), description))
        .collect()
}
fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_budget_is_enforced_before_metadata_is_copied_out() {
        let fields: Vec<_> = (0..200)
            .map(|index| {
                (
                    format!("field_{index:03}_{}", "x".repeat(6000)),
                    Value::Int(0),
                )
            })
            .collect();
        let names = candidates(fields.iter().map(|(name, value)| (name, value)), "");
        assert!(!names.is_empty());
        assert!(names.len() < fields.len());
        let bytes: usize = names
            .iter()
            .map(|(name, kind)| name.len() + kind.len())
            .sum();
        assert!(bytes <= 1024 * 1024);
        assert!(bytes + fields[names.len()].0.len() + "Int".len() > 1024 * 1024);
    }
}
