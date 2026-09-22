//! Compiled lexical layouts share names; calls copy only their traced value slots.
use crate::value::Value;
use gc_arena::{Collect, Gc, Mutation};
use indexmap::IndexSet;
use std::{collections::HashMap, rc::Rc};

#[derive(Clone, Collect)]
#[collect(no_drop)]
pub struct Scope<'gc> {
    #[collect(require_static)]
    pub layout: Rc<IndexSet<String>>,
    pub slots: Vec<Option<Value<'gc>>>,
    // Only entry/module scopes have a snapshot. Closures capture selected values.
    snapshot: Option<Gc<'gc, HashMap<String, Value<'gc>>>>,
}

impl<'gc> Scope<'gc> {
    pub fn new(layout: &Rc<IndexSet<String>>) -> Self {
        Self {
            layout: Rc::clone(layout),
            slots: vec![None; layout.len()],
            snapshot: None,
        }
    }
    pub fn with_snapshot(
        mc: &Mutation<'gc>,
        layout: &Rc<IndexSet<String>>,
        snapshot: HashMap<String, Value<'gc>>,
    ) -> Self {
        Self {
            snapshot: Some(Gc::new(mc, snapshot)),
            ..Self::new(layout)
        }
    }
    pub fn get(&self, name: &str) -> Option<&Value<'gc>> {
        self.layout
            .get_index_of(name)
            .and_then(|slot| self.slots[slot].as_ref())
            .or_else(|| {
                self.snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.get(name))
            })
    }
    pub fn insert(&mut self, name: &str, value: Value<'gc>) {
        let slot = self
            .layout
            .get_index_of(name)
            .expect("compiled binding slot");
        self.slots[slot] = Some(value);
    }
    pub fn extend<S: AsRef<str>>(&mut self, bindings: impl IntoIterator<Item = (S, Value<'gc>)>) {
        for (name, value) in bindings {
            self.insert(name.as_ref(), value);
        }
    }
    /// Previously published snapshots have already passed the escape check.
    pub fn local_values(&self) -> impl Iterator<Item = &Value<'gc>> {
        self.slots.iter().flatten()
    }
    pub fn values(&self) -> impl Iterator<Item = &Value<'gc>> {
        self.slots
            .iter()
            .flatten()
            .chain(self.snapshot.iter().flat_map(|snapshot| snapshot.values()))
    }
    /// Publish only this entry's declarations; its lexical snapshot is not republished.
    pub fn publish(self, bindings: &mut HashMap<String, Value<'gc>>) {
        bindings.extend(
            self.layout
                .iter()
                .zip(self.slots)
                .filter_map(|(name, value)| value.map(|value| (name.clone(), value))),
        );
    }
}
