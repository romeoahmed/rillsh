//! Plan construction validates each argument before evaluation advances to the next.
use crate::{
    Error,
    value::{JobPlan, Value},
};
use gc_arena::{Gc, Mutation};
use rill_system::plan::{Plan, Redirect, Stage};
use std::{ffi::CString, os::unix::ffi::OsStrExt};

pub fn argument(stage: &mut Stage, value: Value<'_>, stage_index: usize) -> Result<(), Error> {
    let bytes = super::native_path(value)
        .map_err(|_| {
            Error::type_error(format!(
                "stage {stage_index} argv[{}] requires String, Bytes, or Path without NUL",
                stage.argv.len()
            ))
        })?
        .as_os_str()
        .as_bytes();
    if stage.argv.is_empty() && bytes.is_empty() {
        return Err(Error::type_error(format!(
            "stage {stage_index} executable must not be empty"
        )));
    }
    stage
        .argv
        .push(CString::new(bytes).map_err(|_| Error::type_error("command argument contains NUL"))?);
    Ok(())
}
pub fn redirect(
    kind: rill_syntax::token::Redirect,
    value: Option<Value<'_>>,
) -> Result<Redirect, Error> {
    use rill_syntax::token::Redirect as Syntax;
    if kind == Syntax::ErrorToOutput {
        return Ok(Redirect::ErrorToOutput);
    }
    let path = super::native_path(value.expect("file redirect has an operand"))?.to_path_buf();
    Ok(match kind {
        Syntax::Input => Redirect::Read(path),
        Syntax::Output | Syntax::Append => Redirect::Write {
            stream: rill_system::plan::Output::Stdout,
            path,
            append: kind == Syntax::Append,
        },
        Syntax::ErrorOutput | Syntax::ErrorAppend => Redirect::Write {
            stream: rill_system::plan::Output::Stderr,
            path,
            append: kind == Syntax::ErrorAppend,
        },
        Syntax::ErrorToOutput => unreachable!("descriptor duplication handled first"),
    })
}
pub fn apply<'gc>(mc: &Mutation<'gc>, request: Value<'gc>) -> Result<Value<'gc>, Error> {
    let request = super::items(request)?;
    let [Value::String(name), args @ ..] = request.as_slice() else {
        return Err(Error::type_error("invalid process primitive request"));
    };
    let plan = match (name.as_str(), args) {
        ("command", [program, Value::List(args)]) => {
            let mut stage = Stage::default();
            argument(&mut stage, *program, 0)?;
            for value in args.as_slice() {
                argument(&mut stage, *value, 0)?;
            }
            Plan {
                stages: vec![stage],
            }
        }
        ("pipe", [Value::Plan(left), Value::Plan(right)]) => {
            let mut plan = left.0.clone();
            plan.stages.extend(right.0.stages.iter().cloned());
            plan
        }
        ("with_env", [Value::Record(fields), Value::Plan(original)]) => {
            let mut environment = std::collections::BTreeMap::new();
            for (name, value) in fields.iter() {
                let name = super::service::environment_key(name)?;
                environment.insert(name, Some(super::service::native_string(*value)?));
            }
            let mut plan = original.0.clone();
            for stage in &mut plan.stages {
                stage.environment.extend(environment.clone());
            }
            plan
        }
        ("without_env", [Value::List(names), Value::Plan(original)]) => {
            let names = names
                .as_slice()
                .iter()
                .map(|value| super::service::environment_name(*value))
                .collect::<Result<Vec<_>, _>>()?;
            let mut plan = original.0.clone();
            for stage in &mut plan.stages {
                stage
                    .environment
                    .extend(names.iter().cloned().map(|name| (name, None)));
            }
            plan
        }
        ("redirect", [Value::String(kind), path, Value::Plan(original)]) => {
            use rill_syntax::token::Redirect as Kind;
            let kind = redirect_kind(kind)?;
            let redirect = redirect(kind, Some(*path))?;
            let mut plan = original.0.clone();
            let stage = if kind == Kind::Input {
                plan.stages.first_mut()
            } else {
                plan.stages.last_mut()
            }
            .ok_or_else(|| Error::type_error("plan has no stages"))?;
            stage.redirects.push(redirect);
            plan
        }
        ("accept_exit", [Value::List(codes), Value::Plan(original)]) => {
            let mut codes = codes
                .as_slice()
                .iter()
                .map(|value| match value {
                    Value::Int(n) => u8::try_from(*n)
                        .map_err(|_| Error::type_error("exit code must be between 0 and 255")),
                    _ => Err(Error::type_error("exit codes require Int")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut plan = original.0.clone();
            if codes.is_empty() {
                return Err(Error::type_error("accepted exit codes must be nonempty"));
            }
            codes.sort_unstable();
            codes.dedup();
            plan.stages
                .last_mut()
                .ok_or_else(|| Error::type_error("plan has no stages"))?
                .accepted_codes = codes;
            plan
        }
        _ => {
            return Err(Error::type_error(format!(
                "invalid or unavailable process operation '{name}'"
            )));
        }
    };
    Ok(Value::Plan(Gc::new(mc, JobPlan(plan))))
}

fn redirect_kind(kind: &str) -> Result<rill_syntax::token::Redirect, Error> {
    use rill_syntax::token::Redirect as Kind;
    Ok(match kind {
        "stdin" => Kind::Input,
        "stdout" => Kind::Output,
        "append_stdout" => Kind::Append,
        "stderr" => Kind::ErrorOutput,
        "append_stderr" => Kind::ErrorAppend,
        "stderr_to_stdout" => Kind::ErrorToOutput,
        _ => return Err(Error::type_error("unknown redirect operation")),
    })
}
