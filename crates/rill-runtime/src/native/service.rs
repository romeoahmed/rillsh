//! Validate service arguments before transferring owned requests to the coordinator.
use crate::{
    Error,
    host::{Request, RunMode},
    value::Value,
};
use std::ffi::CString;

pub fn host(name: &str, value: Value<'_>) -> Result<Request, Error> {
    match (name, value) {
        ("start", Value::Plan(plan)) => Ok(Request::Start(plan.0.clone())),
        ("jobs", Value::Unit) => Ok(Request::Jobs),
        ("wait" | "fg" | "bg" | "cancel", Value::Job(id)) => Ok(Request::Control {
            id,
            operation: match name {
                "wait" => crate::host::Control::Wait,
                "fg" => crate::host::Control::Foreground,
                "bg" => crate::host::Control::Background,
                _ => crate::host::Control::Cancel,
            },
        }),
        ("run" | "execute", Value::Plan(plan)) => Ok(Request::Run {
            plan: plan.0.clone(),
            mode: if name == "run" {
                RunMode::Checked
            } else {
                RunMode::Report
            },
            max_bytes: 0,
        }),
        ("cd", value) => Ok(Request::Cd(super::native_path(value)?.to_path_buf())),
        ("pwd", Value::Unit) => Ok(Request::Pwd),
        ("exit" | "exit_force", Value::Int(code)) => Ok(Request::Exit {
            code: u8::try_from(code)
                .map_err(|_| Error::type_error("exit code must be between 0 and 255"))?,
            force: name == "exit_force",
        }),
        _ => Err(Error::type_error(format!(
            "invalid or unavailable operation '{name}'"
        ))),
    }
}
pub fn process(value: Value<'_>) -> Result<Option<Request>, Error> {
    let values = super::items(value)?;
    let [Value::String(name), args @ ..] = values.as_slice() else {
        return Err(Error::type_error("invalid process request"));
    };
    Ok(match (name.as_str(), args) {
        ("with_cwd", [path, Value::Plan(plan)]) => Some(Request::WithCwd {
            path: super::native_path(*path)?.to_path_buf(),
            plan: plan.0.clone(),
        }),
        ("get_env", [name]) => Some(Request::GetEnv(environment_name(*name)?)),
        ("set_env", [name, value]) => Some(Request::SetEnv {
            name: environment_name(*name)?,
            value: native_string(*value)?,
        }),
        ("unset_env", [name]) => Some(Request::UnsetEnv(environment_name(*name)?)),
        ("args", []) => Some(Request::Args),
        _ => None,
    })
}
pub fn data(value: Value<'_>) -> Result<Option<Request>, Error> {
    let values = super::items(value)?;
    let [Value::String(name), args @ ..] = values.as_slice() else {
        return Err(Error::type_error("invalid data request"));
    };
    Ok(match (name.as_str(), args) {
        ("glob", [pattern]) => Some(Request::Glob(native_string(*pattern)?)),
        ("capture", [options, Value::Plan(plan)]) => Some(Request::Run {
            plan: plan.0.clone(),
            mode: RunMode::Capture,
            max_bytes: byte_limit(*options)?,
        }),
        ("read_text", [options, path]) => Some(Request::ReadText {
            path: super::native_path(*path)?.to_path_buf(),
            max_bytes: byte_limit(*options)?,
        }),
        _ => None,
    })
}
fn environment_name(value: Value<'_>) -> Result<CString, Error> {
    let bytes = native_string(value)?;
    if bytes.as_bytes().is_empty() || bytes.as_bytes().contains(&b'=') {
        return Err(Error::type_error(
            "environment name must be nonempty and contain neither '=' nor NUL",
        ));
    }
    Ok(bytes)
}
pub fn byte_limit(value: Value<'_>) -> Result<usize, Error> {
    let Value::Record(fields) = value else {
        return Err(Error::type_error("options require Record"));
    };
    let mut limit = 64 * 1024 * 1024;
    for (name, value) in fields.iter() {
        let Value::Int(n) = value else {
            return Err(Error::type_error("byte limit requires Int"));
        };
        if name != "max_bytes" {
            return Err(Error::type_error(format!("unknown limit '{name}'")));
        }
        limit =
            usize::try_from(*n).map_err(|_| Error::type_error("byte limit must be nonnegative"))?;
    }
    Ok(limit)
}

pub fn native_string(value: Value<'_>) -> Result<CString, Error> {
    let bytes = match &value {
        Value::String(text) => text.as_bytes(),
        Value::Bytes(bytes) => &bytes.0,
        _ => {
            return Err(Error::type_error("native string requires String or Bytes"));
        }
    };
    CString::new(bytes).map_err(|_| Error::type_error("native string contains NUL"))
}
