//! Module inputs are opened outside arena mutation; only identities enter the heap.
use crate::{Error, Source, code::Code, compiler};
use gc_arena::Collect;
use rill_syntax::{
    ast::{Module, Statement},
    token::Span,
};
use rill_system::source::{Directory, Identity, SourceFile};
use std::{collections::HashMap, fs::File, rc::Rc};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Collect)]
#[collect(require_static)]
pub enum Key {
    File(Identity),
    Bundled(&'static str),
}

pub struct Request {
    pub path: String,
    pub name: String,
    pub context: Option<usize>,
    pub source: Rc<Source>,
    pub span: Span,
}

#[derive(Default)]
pub struct Loader {
    directories: HashMap<usize, Directory>,
    next: usize,
    pending: HashMap<Identity, File>,
    retained: HashMap<Identity, File>,
    suspended: HashMap<i64, HashMap<Identity, File>>,
}

impl Loader {
    pub fn suspend(&mut self, id: i64) {
        self.suspended.insert(id, std::mem::take(&mut self.pending));
    }
    pub fn activate(&mut self, id: i64) {
        self.pending = self.suspended.remove(&id).unwrap_or_default();
    }

    pub fn publish(&mut self, keys: impl IntoIterator<Item = Key>) {
        for key in keys {
            if let Key::File(identity) = key
                && let Some(file) = self.pending.remove(&identity)
            {
                self.retained.insert(identity, file);
            }
        }
    }
    pub fn abort(&mut self) {
        self.pending.clear();
    }
    pub fn context(&mut self, module: &Module, directory: Directory) -> Option<usize> {
        if !needs_directory(module) {
            return None;
        }
        let key = self.next;
        self.next = self
            .next
            .checked_add(1)
            .expect("source directory identity space exhausted");
        self.directories.insert(key, directory);
        Some(key)
    }
    pub fn release(&mut self, contexts: impl IntoIterator<Item = usize>) {
        for context in contexts {
            self.directories.remove(&context);
        }
    }
    pub fn directory(&self, request: &Request) -> Result<Directory, Error> {
        let directory = self
            .directories
            .get(&request.context.ok_or_else(|| {
                Error::new(
                    "ImportError",
                    "bundled modules cannot import filesystem paths",
                )
            })?)
            .expect("active source owns its directory");
        directory.try_clone().map_err(Error::from)
    }
    pub fn compile(
        &mut self,
        request: &Request,
        file: SourceFile,
        text: String,
    ) -> Result<Rc<Code>, Error> {
        let module = rill_syntax::parse(&request.path, &text).map_err(|errors| {
            let first = errors
                .into_iter()
                .next()
                .expect("failed parse has a diagnostic");
            let mut error = Error::new("ParseError", first.message);
            error.span = Some(first.span);
            error.origin = Some(Rc::new(Source {
                name: request.path.clone(),
                text,
            }));
            error
        })?;
        let identity = file.identity;
        let (directory, file) = file.into_parts();
        let context = self.context(&module, directory);
        let code = compiler::compile(&module, context);
        self.pending.insert(identity, file);
        Ok(code)
    }
}

/// Bundled module names never resolve against a filesystem capability.
pub fn needs_directory(module: &Module) -> bool {
    module.statements.iter().any(
        |statement| matches!(statement, Statement::Import { path, .. } if !path.starts_with("std:")),
    )
}

/// The bundle owns the sources; imports share the ordinary module cache and VM frames.
pub fn bundle(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "std:core" => Some(("std:core", include_str!("../stdlib/core.rill"))),
        "std:option" => Some(("std:option", include_str!("../stdlib/option.rill"))),
        "std:result" => Some(("std:result", include_str!("../stdlib/result.rill"))),
        "std:seq" => Some(("std:seq", include_str!("../stdlib/seq.rill"))),
        "std:text" => Some(("std:text", include_str!("../stdlib/text.rill"))),
        "std:fs" => Some(("std:fs", include_str!("../stdlib/fs.rill"))),
        "std:json" => Some(("std:json", include_str!("../stdlib/json.rill"))),
        "std:process" => Some(("std:process", include_str!("../stdlib/process.rill"))),
        "std:prelude" => Some(("std:prelude", include_str!("../stdlib/prelude.rill"))),
        _ => None,
    }
}
