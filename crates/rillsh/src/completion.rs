//! One short-lived filesystem worker per editor, with bounded results and explicit reaping.
use rill_editor::completion::{Request, Target};
use rill_system::{
    job::{Job, LaunchMode, Snapshot, read_chunk},
    plan::{Plan, Stage},
};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsString},
    io::{self, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::PermissionsExt,
    },
    path::{Path, PathBuf},
    time::Duration,
};

const BYTES: usize = 1024 * 1024;
const CANDIDATES: usize = 200;

pub async fn complete(
    mut request: Request,
    launcher: PathBuf,
    snapshot: Snapshot,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) {
    let Target::Paths(query) = &request.target else {
        return;
    };
    let command = query.command;
    let Ok(prefix) = CString::new(query.prefix.as_bytes()) else {
        return;
    };
    if prefix.as_bytes().len() > 8192 {
        return;
    }
    let Ok(executable) = CString::new(launcher.as_os_str().as_bytes()) else {
        return;
    };
    let plan = Plan {
        stages: vec![Stage {
            argv: vec![
                executable,
                c"--internal-complete".into(),
                if query.executable {
                    c"executable"
                } else {
                    c"path"
                }
                .into(),
                prefix,
            ],
            ..Stage::default()
        }],
    };
    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    let Ok(mut job) = Job::prepare(&launcher, &plan, &snapshot, LaunchMode::Capture).await else {
        return;
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, collect(&mut job)) => result.ok().and_then(Result::ok),
        () = request.cancelled() => None,
        _ = &mut cancel => None,
    };
    // Even cancelled and expired queries remain owned until their worker is reaped.
    let cleaned = job.cancel().await.is_ok();
    if cleaned && !request.is_cancelled() && tokio::time::Instant::now() <= deadline {
        let candidates = result
            .and_then(|bytes| decode(&bytes, command).ok())
            .unwrap_or_default();
        request.respond(candidates);
    }
}

async fn collect(job: &mut Job) -> io::Result<Vec<u8>> {
    job.commit().await?;
    let stdout = job
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("completion output unavailable"))?;
    let mut bytes = Vec::new();
    loop {
        let chunk = read_chunk(&stdout).await?;
        if chunk.is_empty() {
            break;
        }
        if chunk.len() > BYTES.saturating_sub(bytes.len()) {
            return Err(io::Error::other("completion output limit exceeded"));
        }
        bytes.extend(chunk);
    }
    job.wait().await?;
    if job
        .terminations()
        .iter()
        .any(|status| *status != Some(rill_system::job::Termination::Exited(0)))
    {
        return Err(io::Error::other("completion worker failed"));
    }
    Ok(bytes)
}

fn decode(bytes: &[u8], command: bool) -> io::Result<Vec<(String, String)>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let Some(bytes) = bytes.strip_suffix(&[0]) else {
        return Err(io::Error::other("incomplete completion response"));
    };
    let mut total: usize = 0;
    bytes
        .split(|byte| *byte == 0)
        .map(|record| {
            let Some((kind, path)) = record.split_first() else {
                return Err(io::Error::other("empty completion response"));
            };
            let kind = match kind {
                b'd' => "Directory",
                b'f' => "Path",
                b'x' => "Executable",
                _ => return Err(io::Error::other("invalid completion response")),
            };
            let source = rill_syntax::completion::path_source(path, command);
            total = total
                .checked_add(source.len() + kind.len())
                .filter(|size| *size <= BYTES)
                .ok_or_else(|| io::Error::other("completion text limit exceeded"))?;
            Ok((source, kind.into()))
        })
        .collect()
}

/// Runs before any coordinator starts; all filesystem traversal stays in this process.
pub fn worker(mut arguments: impl Iterator<Item = OsString>) -> io::Result<()> {
    let executable = arguments.next().is_some_and(|mode| mode == "executable");
    let prefix = arguments
        .next()
        .filter(|_| arguments.next().is_none())
        .ok_or_else(|| io::Error::other("invalid completion request"))?;
    let mut selected = BTreeMap::new();
    let mut retained = 0;
    if executable && !prefix.as_bytes().contains(&b'/') {
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        for directory in std::env::split_paths(&path) {
            scan(
                if directory.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    &directory
                },
                &prefix,
                b"",
                Selection::Executables,
                &mut selected,
                &mut retained,
            );
        }
    } else {
        let bytes = prefix.as_bytes();
        let split = bytes
            .iter()
            .rposition(|byte| *byte == b'/')
            .map_or(0, |index| index + 1);
        let (directory, name) = bytes.split_at(split);
        let path = PathBuf::from(OsString::from_vec(directory.into()));
        scan(
            if directory.is_empty() {
                Path::new(".")
            } else {
                &path
            },
            std::ffi::OsStr::from_bytes(name),
            directory,
            if executable {
                Selection::CommandPath
            } else {
                Selection::Paths
            },
            &mut selected,
            &mut retained,
        );
    }
    let mut output = io::stdout().lock();
    for (path, kind) in selected {
        output.write_all(&[kind])?;
        output.write_all(&path)?;
        output.write_all(&[0])?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    Paths,
    Executables,
    CommandPath,
}

fn scan(
    directory: &Path,
    prefix: &std::ffi::OsStr,
    base: &[u8],
    selection: Selection,
    selected: &mut BTreeMap<Vec<u8>, u8>,
    retained: &mut usize,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.as_bytes().starts_with(prefix.as_bytes())
            || (name.as_bytes().starts_with(b".") && !prefix.as_bytes().starts_with(b"."))
        {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let kind = if metadata.is_dir() && selection != Selection::Executables {
            b'd'
        } else if selection != Selection::Paths
            && metadata.is_file()
            && metadata.permissions().mode() & 0o111 != 0
        {
            b'x'
        } else if selection != Selection::Paths {
            continue;
        } else {
            b'f'
        };
        let mut path = base.to_vec();
        path.extend(name.as_bytes());
        if kind == b'd' {
            path.push(b'/');
        }
        if selected.contains_key(&path) || path.len() + 2 > BYTES.saturating_sub(*retained) {
            continue;
        }
        *retained += path.len() + 2;
        selected.insert(path, kind);
        if selected.len() > CANDIDATES
            && let Some((path, _)) = selected.pop_last()
        {
            *retained -= path.len() + 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_preserves_native_bytes_without_source_injection() {
        let candidates = decode(b"fa\xff\0f$(danger)\0dspace name/\0", true).unwrap();
        assert_eq!(candidates[0].0, "$(path (bytes [97, 255]))");
        assert_eq!(candidates[1].0, "\"$(danger)\"");
        assert_eq!(candidates[2].0, "\"space name/\"");
        assert!(decode(b"broken", true).is_err());
    }
}
