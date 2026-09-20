//! Native globbing runs in an owned process with the session's directory capability.
use super::Session;
use rill_runtime::{
    Error,
    host::{Response, RunMode},
};
use rill_system::{
    job::Termination,
    plan::{Plan, Stage},
    resources::SourceError,
};
use std::{
    ffi::{CString, OsString},
    io,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};

// Bound materialized IPC output independently of libc's private traversal storage.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

impl Session {
    #[expect(
        clippy::future_not_send,
        reason = "The coordinator retains the glob helper until its response is complete"
    )]
    pub(super) async fn glob(&mut self, pattern: CString) -> Result<Response, Error> {
        let run = self.prepare_glob(pattern).await?;
        let response = self.continue_run(run).await?;
        self.glob_response(response)
    }

    #[expect(
        clippy::future_not_send,
        reason = "The VM coordinator runs on the current-thread runtime"
    )]
    pub(super) async fn prepare_glob(
        &mut self,
        pattern: CString,
    ) -> Result<super::execution::Running, Error> {
        let argv = [
            CString::new(self.launcher.as_os_str().as_bytes())
                .map_err(|e| Error::from_io(&e.into()))?,
            c"--internal-glob".into(),
            pattern,
        ];
        let plan = Plan {
            stages: vec![Stage {
                argv: argv.into(),
                ..Stage::default()
            }],
        };
        self.prepare_run(plan, RunMode::Capture, MAX_RESPONSE_BYTES)
            .await
    }
    pub(super) fn glob_response(&mut self, response: Response) -> Result<Response, Error> {
        if self.suspend_requested {
            if let Some(super::continuation::Pending::Run(run)) = self.pending.take() {
                self.pending = Some(super::continuation::Pending::Glob(run));
            }
            return Ok(Response::Unit);
        }
        self::response(response).map_err(Error::from)
    }
}

pub(super) fn response(value: Response) -> Result<Response, SourceError> {
    let Response::Run {
        stdout,
        stderr,
        terminations,
        ..
    } = value
    else {
        unreachable!("captured glob helper result")
    };
    let [termination] = terminations.as_slice() else {
        unreachable!("one glob helper has one termination status")
    };
    if *termination != Termination::Exited(0) {
        let message = match termination {
            Termination::Exited(code) => format!("glob helper exited with status {code}"),
            Termination::Signaled(signal) => {
                format!("glob helper terminated by signal {signal}")
            }
        };
        let mut notes = Vec::new();
        let detail = String::from_utf8_lossy(&stderr);
        if !detail.trim_end().is_empty() {
            notes.push(detail.trim_end().into());
        }
        return Err(SourceError::Cleanup {
            primary: Box::new(io::Error::other(message).into()),
            notes,
        });
    }
    let paths = decode(&stdout, MAX_RESPONSE_BYTES)?;
    Ok(Response::Paths(paths))
}

fn decode(bytes: &[u8], limit: usize) -> Result<Vec<PathBuf>, SourceError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut retained = bytes.len();
    let bytes = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "incomplete glob response"))?;
    bytes
        .split(|byte| *byte == 0)
        .map(|path| {
            if path.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "empty path in glob response",
                )
                .into());
            }
            retained = retained
                .checked_add(size_of::<PathBuf>())
                .filter(|size| *size <= limit)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::FileTooLarge,
                        "glob results exceed the materialization limit",
                    )
                })?;
            Ok(PathBuf::from(OsString::from_vec(path.into())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_path_records_preserve_bytes_and_reject_truncation() {
        let paths = decode(b"a\xff\0line\nname\0", 1024).unwrap();
        assert_eq!(paths[0].as_os_str().as_bytes(), b"a\xff");
        assert_eq!(paths[1].as_os_str().as_bytes(), b"line\nname");
        assert!(decode(b"missing-terminator", 1024).is_err());
        assert!(decode(b"\0", 1024).is_err());
        assert!(decode(b"", 0).unwrap().is_empty());
        assert_eq!(
            Error::from(decode(b"a\0b\0", 4).unwrap_err()).kind,
            "LimitExceeded"
        );
    }
}
