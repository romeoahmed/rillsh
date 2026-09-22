//! Check immutable completion data; the launcher, not check, selects the failing stage.
use crate::{Error, value::Value};
use gc_arena::Gc;

fn instance<'gc>(value: Value<'gc>, constructor: Value<'gc>) -> bool {
    let Value::Adt(value) = value else {
        return false;
    };
    match constructor {
        Value::Constructor(descriptor) => Gc::ptr_eq(value.descriptor, descriptor),
        Value::Adt(case) => Gc::ptr_eq(value.descriptor, case.descriptor),
        _ => false,
    }
}

pub fn check<'gc>(
    process: Value<'gc>,
    library: Value<'gc>,
    report: Value<'gc>,
) -> Result<(), Error> {
    if !instance(report, process.field("JobReport")?) {
        return Err(Error::type_error(
            "check requires the shared JobReport type",
        ));
    }
    let completion = report.field("completion")?;
    let cases = process.field("Completion")?;
    let cancelled = instance(completion, cases.field("Cancelled")?);
    if cancelled || instance(completion, cases.field("Cutoff")?) {
        if !matches!(completion.field("reason")?, Value::String(_)) {
            return Err(Error::type_error("completion reason requires String"));
        }
    } else if !instance(completion, cases.field("Finished")?) {
        return Err(Error::type_error("invalid Completion type"));
    }
    if cancelled {
        return Err(Error::new("ProcessError", "job was cancelled"));
    }
    let failure = report.field("failure")?;
    let option = library.field("Option")?;
    if instance(failure, option.field("None")?) {
        return Ok(());
    }
    if !instance(failure, option.field("Some")?) {
        return Err(Error::type_error("report failure requires Option"));
    }
    let Value::Int(index) = failure.field("value")? else {
        return Err(Error::type_error("failed stage index requires Int"));
    };
    let stages = super::items(report.field("stages")?)?;
    let index =
        usize::try_from(index).map_err(|_| Error::type_error("invalid failed stage index"))?;
    let stage = stages
        .as_slice()
        .get(index)
        .ok_or_else(|| Error::type_error("failed stage index is outside the report"))?;
    let termination = stage.field("termination")?;
    let cases = process.field("Termination")?;
    let code = if instance(termination, cases.field("Exited")?) {
        let Value::Int(code) = termination.field("code")? else {
            return Err(Error::type_error("exit code requires Int"));
        };
        u8::try_from(code)
            .map_err(|_| Error::type_error("exit code is outside 0..255"))?
            .max(1)
    } else if instance(termination, cases.field("Signaled")?) {
        let Value::Int(signal) = termination.field("signal")? else {
            return Err(Error::type_error("signal requires Int"));
        };
        if signal <= 0 {
            return Err(Error::type_error("signal must be positive"));
        }
        u8::try_from(signal.saturating_add(128).min(255)).map_err(|_| Error::arithmetic())?
    } else {
        return Err(Error::type_error("invalid Termination type"));
    };
    let mut error = Error::new(
        "ProcessError",
        format!("stage {index} failed with status {code}"),
    );
    error.exit_status = Some(code);
    error.details.insert(
        "stage".into(),
        crate::error::Detail::Int(i64::try_from(index).unwrap_or(i64::MAX)),
    );
    Err(error)
}
