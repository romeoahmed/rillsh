//! Resumable native computations share instruction fuel and traced frame ownership.
use crate::{Error, equality::Equality, value::Value};
use gc_arena::{Collect, Mutation};

#[derive(Collect)]
#[collect(no_drop)]
pub enum Work<'gc> {
    Equality(Equality<'gc>),
    Sort(super::sort::Work<'gc>),
    Collection(super::collection::Work<'gc>),
    Json(super::json::Work<'gc>),
}
impl<'gc> Work<'gc> {
    pub fn advance(
        &mut self,
        mc: &Mutation<'gc>,
        fuel: &mut usize,
    ) -> Result<Option<Value<'gc>>, Error> {
        match self {
            Self::Equality(work) => work.advance(fuel).map(|value| value.map(Value::Bool)),
            Self::Sort(work) => work.advance(mc, fuel),
            Self::Collection(work) => work.advance(mc, fuel),
            Self::Json(work) => work.advance(mc, fuel),
        }
    }
}
