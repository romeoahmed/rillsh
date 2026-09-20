//! Strict UTF-8 line decoding retains only a partial line and the current shared chunk.
use crate::{Error, value::Value};
use gc_arena::{Collect, Mutation};

#[derive(Collect)]
#[collect(no_drop)]
pub struct Lines<'gc> {
    pub chunk: Option<Value<'gc>>,
    pub ended: bool,
    cursor: usize,
    pending: Vec<u8>,
    limit: usize,
}
pub enum Poll<'gc> {
    Item(Value<'gc>),
    NeedInput,
    End,
    Continue,
}
impl<'gc> Lines<'gc> {
    pub fn new(options: Value<'_>) -> Result<Self, Error> {
        let Value::Record(options) = options else {
            return Err(Error::type_error("line options require Record"));
        };
        let mut limit = 8 * 1024 * 1024;
        for (name, value) in options.iter() {
            if name != "max_line_bytes" {
                return Err(Error::type_error(format!("unknown line option '{name}'")));
            }
            let Value::Int(count) = value else {
                return Err(Error::type_error("line limit requires Int"));
            };
            limit = usize::try_from(*count)
                .map_err(|_| Error::type_error("line limit must be nonnegative"))?;
        }
        Ok(Self {
            chunk: None,
            ended: false,
            cursor: 0,
            pending: Vec::new(),
            limit,
        })
    }
    pub fn poll(&mut self, mc: &Mutation<'gc>) -> Result<Poll<'gc>, Error> {
        if let Some(Value::Bytes(chunk)) = self.chunk {
            let available = &chunk.0[self.cursor..];
            let quantum = available.len().min(16 * 1024);
            let newline = available[..quantum].iter().position(|byte| *byte == b'\n');
            let count = newline.unwrap_or(quantum);
            if count > self.limit.saturating_sub(self.pending.len()) {
                return Err(Error::new("LimitExceeded", "line exceeds the byte limit"));
            }
            self.pending.extend_from_slice(&available[..count]);
            self.cursor += count + usize::from(newline.is_some());
            if self.cursor == chunk.0.len() {
                self.chunk = None;
                self.cursor = 0;
            }
            if newline.is_some() {
                if self.pending.last() == Some(&b'\r') {
                    self.pending.pop();
                }
                return self.line(mc).map(Poll::Item);
            }
            return Ok(Poll::Continue);
        }
        if self.ended {
            if self.pending.is_empty() {
                Ok(Poll::End)
            } else {
                self.line(mc).map(Poll::Item)
            }
        } else {
            Ok(Poll::NeedInput)
        }
    }
    fn line(&mut self, mc: &Mutation<'gc>) -> Result<Value<'gc>, Error> {
        String::from_utf8(std::mem::take(&mut self.pending))
            .map(|text| Value::string(mc, text))
            .map_err(|error| Error::new("DecodeError", error.to_string()))
    }
}
