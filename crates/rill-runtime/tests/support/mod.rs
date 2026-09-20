//! Drive pure entries through the public protocol, collecting at every yield.
use rill_runtime::{Engine, Error, Progress};

pub fn run(engine: &mut Engine, source: &str) -> Result<(), Error> {
    engine.begin(&rill_syntax::parse("test", source).unwrap())?;
    finish(engine)
}

pub fn finish(engine: &mut Engine) -> Result<(), Error> {
    loop {
        match engine.step(32)? {
            Progress::Complete => return Ok(()),
            Progress::Yielded => engine.collect(),
            Progress::Waiting => panic!("pure fixture requested host I/O"),
        }
    }
}
