//! Allocation-heavy collection conversions yield between elements, with arena-owned roots.
use crate::{
    Error,
    operations::list,
    value::{Record, Value},
};
use gc_arena::{Collect, Mutation};

#[derive(Collect)]
#[collect(no_drop)]
pub struct Work<'gc> {
    input: Value<'gc>,
    other: Option<Value<'gc>>,
    kind: Kind,
    index: usize,
    values: Vec<Value<'gc>>,
    fields: Record<'gc>,
    text: String,
}
#[derive(Collect, Clone, Copy)]
#[collect(require_static)]
enum Kind {
    Reverse,
    Concat,
    Take(usize),
    Unroll,
    Record,
    Entries,
    Scalars,
    Join,
    Split,
}
impl<'gc> Work<'gc> {
    pub fn new(argument: Value<'gc>) -> Result<Option<Self>, Error> {
        let args = super::items(argument)?;
        let (kind, input, other) = match args.as_slice() {
            [Value::String(name), args @ ..] => match (name.as_str(), args) {
                ("reverse", [input @ Value::List(_)]) => (Kind::Reverse, *input, None),
                ("concat", [a @ Value::List(_), b @ Value::List(_)]) => {
                    (Kind::Concat, *a, Some(*b))
                }
                ("take", [Value::Int(count), input @ Value::List(_)]) => (
                    Kind::Take(
                        usize::try_from(*count)
                            .map_err(|_| Error::type_error("count must be nonnegative"))?,
                    ),
                    *input,
                    None,
                ),
                ("unroll", [input]) => (Kind::Unroll, *input, None),
                ("record", [input @ Value::List(_)]) => (Kind::Record, *input, None),
                ("entries", [input @ Value::Record(_)]) => (Kind::Entries, *input, None),
                ("scalars", [input @ Value::String(_)]) => (Kind::Scalars, *input, None),
                ("join", [separator @ Value::String(_), input @ Value::List(_)]) => {
                    (Kind::Join, *input, Some(*separator))
                }
                ("split", [separator @ Value::String(text), input @ Value::String(_)]) => {
                    if text.is_empty() {
                        return Err(Error::type_error("split delimiter must not be empty"));
                    }
                    (Kind::Split, *input, Some(*separator))
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        Ok(Some(Self {
            input,
            other,
            kind,
            index: 0,
            values: Vec::new(),
            fields: Record::new(),
            text: String::new(),
        }))
    }
    pub fn advance(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<Option<Value<'gc>>, Error> {
        while *fuel != 0 {
            *fuel -= 1;
            let done = match self.kind {
                Kind::Unroll => {
                    let chain = super::items(self.input)?;
                    match chain.as_slice() {
                        [] => true,
                        [head, tail] => {
                            self.values.push(*head);
                            self.input = *tail;
                            false
                        }
                        _ => return Err(Error::type_error("invalid list accumulation chain")),
                    }
                }
                Kind::Scalars | Kind::Split => self.string_item(mc),
                Kind::Entries => {
                    let Value::Record(fields) = self.input else {
                        unreachable!("entries input")
                    };
                    if let Some((name, value)) = fields.get_index(self.index) {
                        self.values
                            .push(list(mc, vec![Value::string(mc, name.clone()), *value]));
                        self.index += 1;
                        false
                    } else {
                        true
                    }
                }
                _ => self.list_item()?,
            };
            if done {
                let value = match self.kind {
                    Kind::Record => {
                        Value::Record(crate::heap::record(mc, std::mem::take(&mut self.fields)))
                    }
                    Kind::Join => Value::string(mc, std::mem::take(&mut self.text)),
                    Kind::Unroll => {
                        // The language builds a reverse chain; reverse in place without allocating.
                        self.values.reverse();
                        list(mc, std::mem::take(&mut self.values))
                    }
                    _ => list(mc, std::mem::take(&mut self.values)),
                };
                return Ok(Some(value));
            }
        }
        Ok(None)
    }
    fn list_item(&mut self) -> Result<bool, Error> {
        let Value::List(items) = self.input else {
            unreachable!("collection list input")
        };
        let values = items.as_slice();
        let next = match self.kind {
            Kind::Reverse => values
                .len()
                .checked_sub(self.index + 1)
                .and_then(|index| values.get(index)),
            Kind::Take(count) if self.index >= count => None,
            _ => values.get(self.index),
        };
        let Some(value) = next.copied() else {
            if matches!(self.kind, Kind::Concat)
                && let Some(other) = self.other.take()
            {
                self.input = other;
                self.index = 0;
                return Ok(false);
            }
            return Ok(true);
        };
        match self.kind {
            Kind::Record => {
                let pair = super::items(value)?;
                let [Value::String(name), value] = pair.as_slice() else {
                    return Err(Error::type_error(
                        "record entries must be [String, value] pairs",
                    ));
                };
                if self.fields.insert((**name).clone(), *value).is_some() {
                    return Err(Error::type_error(format!("duplicate field '{name}'")));
                }
            }
            Kind::Join => {
                let Value::String(text) = value else {
                    return Err(Error::type_error("join requires a List of Strings"));
                };
                let Some(Value::String(separator)) = self.other else {
                    unreachable!("join separator")
                };
                if self.index != 0 {
                    self.text.push_str(&separator);
                }
                self.text.push_str(&text);
            }
            _ => self.values.push(value),
        }
        self.index += 1;
        Ok(false)
    }
    fn string_item(&mut self, mc: &Mutation<'gc>) -> bool {
        let Value::String(text) = self.input else {
            unreachable!("text input")
        };
        let Some(rest) = text.get(self.index..) else {
            return true;
        };
        if matches!(self.kind, Kind::Scalars) {
            let Some(scalar) = rest.chars().next() else {
                return true;
            };
            self.index += scalar.len_utf8();
            self.values.push(Value::string(mc, scalar.to_string()));
        } else {
            let Some(Value::String(separator)) = self.other else {
                unreachable!("split separator")
            };
            let end = rest.find(separator.as_str()).unwrap_or(rest.len());
            self.values.push(Value::string(mc, &rest[..end]));
            // One-past-end records that the final (possibly empty) field was emitted.
            self.index = if end == rest.len() {
                text.len() + 1
            } else {
                self.index + end + separator.len()
            };
        }
        false
    }
}
