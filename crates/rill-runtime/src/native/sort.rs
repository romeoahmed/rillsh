//! Stable decorate-sort-undecorate with one retained-value budget across keys and input.
use super::budget::Budget;
use crate::{Error, operations::list, value::Value};
use gc_arena::{Collect, Gc, Mutation, RefLock};
use std::cmp::Ordering;

pub fn apply<'gc>(
    mc: &Mutation<'gc>,
    name: &str,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error> {
    match (name, args) {
        ("sort_input", [Value::Record(options), Value::List(items)]) => {
            let mut max_items = 1_000_000;
            let mut max_bytes = 64 * 1024 * 1024;
            for (name, value) in options.iter() {
                let Value::Int(value) = value else {
                    return Err(Error::type_error("sort limits require Int"));
                };
                let value = usize::try_from(*value)
                    .map_err(|_| Error::type_error("sort limits must be nonnegative"))?;
                match name.as_str() {
                    "max_items" => max_items = value,
                    "max_bytes" => max_bytes = value,
                    _ => return Err(Error::type_error(format!("unknown sort option '{name}'"))),
                }
            }
            if items.as_slice().len() > max_items {
                return Err(Error::new(
                    "LimitExceeded",
                    "sort input exceeds the item limit",
                ));
            }
            let mut budget = Budget::new(max_bytes);
            if !items.as_slice().is_empty() {
                budget.value(Value::List(*items))?;
            }
            Ok(Value::Budget(Gc::new(mc, RefLock::new(budget))))
        }
        ("sort_key", [Value::Budget(budget), key]) => {
            let mut budget = budget.borrow_mut(mc);
            if !matches!(key, Value::Int(_) | Value::Float(_) | Value::String(_))
                || budget.kind.is_some_and(|kind| kind != key.kind())
            {
                return Err(Error::type_error("sort keys require one comparable kind"));
            }
            budget.kind = Some(key.kind());
            budget.value(*key)?;
            Ok(*key)
        }
        _ => Err(Error::type_error(
            "sort_by requires options, a key function and a List or Stream",
        )),
    }
}
fn compare(a: Value<'_>, b: Value<'_>) -> Ordering {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => a.cmp(&b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(&b).expect("finite Float"),
        (Value::String(a), Value::String(b)) => a.as_str().cmp(b.as_str()),
        _ => unreachable!("validated comparable keys"),
    }
}

// The standard sorter handles short runs; explicit merging supplies checkpoints that
// one whole-slice sort cannot provide. Left-before-right ties preserve stability.
const RUN: usize = 32;
#[derive(Collect)]
#[collect(no_drop)]
pub struct Work<'gc> {
    input: crate::value::List<'gc>,
    pairs: Vec<(Value<'gc>, Value<'gc>)>,
    scratch: Vec<(Value<'gc>, Value<'gc>)>,
    output: Vec<Value<'gc>>,
    phase: Phase,
}
#[derive(Collect)]
#[collect(require_static)]
enum Phase {
    Build,
    Runs(usize),
    Merge {
        width: usize,
        start: usize,
        left: usize,
        right: usize,
    },
    Output,
}
impl<'gc> Work<'gc> {
    pub fn new(argument: Value<'gc>) -> Result<Option<Self>, Error> {
        let request = super::items(argument)?;
        let [Value::String(name), Value::List(input)] = request.as_slice() else {
            return Ok(None);
        };
        if name.as_str() != "sort" {
            return Ok(None);
        }
        Ok(Some(Self {
            input: *input,
            pairs: Vec::new(),
            scratch: Vec::new(),
            output: Vec::new(),
            phase: Phase::Build,
        }))
    }
    pub fn advance(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<Option<Value<'gc>>, Error> {
        while *fuel != 0 {
            *fuel -= 1;
            match &mut self.phase {
                Phase::Build => {
                    if let Some(pair) = self.input.as_slice().get(self.pairs.len()) {
                        let pair = super::items(*pair)?;
                        let [key, value] = pair.as_slice() else {
                            return Err(Error::type_error("invalid sort decoration"));
                        };
                        if !matches!(key, Value::Int(_) | Value::Float(_) | Value::String(_))
                            || self
                                .pairs
                                .first()
                                .is_some_and(|(first, _)| first.kind() != key.kind())
                        {
                            return Err(Error::type_error("sort keys require one comparable kind"));
                        }
                        self.pairs.push((*key, *value));
                    } else {
                        self.phase = Phase::Runs(0);
                    }
                }
                Phase::Runs(start) => {
                    let end = (*start + RUN).min(self.pairs.len());
                    self.pairs[*start..end].sort_by(|(a, _), (b, _)| compare(*a, *b));
                    if end == self.pairs.len() {
                        self.phase = if self.pairs.len() <= RUN {
                            Phase::Output
                        } else {
                            Phase::Merge {
                                width: RUN,
                                start: 0,
                                left: 0,
                                right: RUN,
                            }
                        };
                    } else {
                        *start = end;
                    }
                }
                Phase::Merge {
                    width,
                    start,
                    left,
                    right,
                } => {
                    let middle = (*start + *width).min(self.pairs.len());
                    let end = middle.saturating_add(*width).min(self.pairs.len());
                    if *left < middle
                        && (*right == end
                            || compare(self.pairs[*left].0, self.pairs[*right].0)
                                != Ordering::Greater)
                    {
                        self.scratch.push(self.pairs[*left]);
                        *left += 1;
                    } else if *right < end {
                        self.scratch.push(self.pairs[*right]);
                        *right += 1;
                    } else if end < self.pairs.len() {
                        *start = end;
                        *left = end;
                        *right = end.saturating_add(*width).min(self.pairs.len());
                    } else {
                        std::mem::swap(&mut self.pairs, &mut self.scratch);
                        self.scratch.clear();
                        *width = width.saturating_mul(2);
                        if *width >= self.pairs.len() {
                            self.phase = Phase::Output;
                        } else {
                            *start = 0;
                            *left = 0;
                            *right = *width;
                        }
                    }
                }
                Phase::Output => {
                    if let Some((_, value)) = self.pairs.get(self.output.len()) {
                        self.output.push(*value);
                    } else {
                        return Ok(Some(list(mc, std::mem::take(&mut self.output))));
                    }
                }
            }
        }
        Ok(None)
    }
}
