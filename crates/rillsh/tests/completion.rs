//! Filesystem candidates round-trip through actual Rill commands without interpretation.
use std::{
    ffi::OsString,
    fs,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    process::Command,
};

#[test]
fn native_filenames_complete_as_exactly_one_argument() {
    let directory = tempfile::tempdir().unwrap();
    let names: Vec<&[u8]> = [
        b"file space".as_slice(),
        b"file$(raise)",
        b"file\"quote",
        b"file'quote",
        b"file\\backslash",
        b"file\nline",
    ]
    .into_iter()
    // Native macOS filesystems reject ill-formed UTF-8 filenames. Argument bytes
    // and the completion protocol still exercise that case on both platforms.
    .chain(cfg!(target_os = "linux").then_some(b"file\xff".as_slice()))
    .collect();
    for name in &names {
        fs::write(
            directory.path().join(OsString::from_vec(name.to_vec())),
            b"",
        )
        .unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["--internal-complete", "path", "file"])
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let paths: Vec<_> = output
        .stdout
        .strip_suffix(&[0])
        .unwrap()
        .split(|byte| *byte == 0)
        .map(|record| {
            assert_eq!(record[0], b'f');
            &record[1..]
        })
        .collect();
    assert_eq!(paths.len(), names.len());
    assert!(paths.iter().all(|path| names.contains(path)));
    for path in paths.into_iter().chain([b"file\xff".as_slice()]) {
        let source = format!(
            "^printf %s {}",
            rill_syntax::completion::path_source(path, true)
        );
        let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
            .args(["-c", &source])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, path);
    }
}

#[test]
fn executable_search_deduplicates_path_entries_and_filters_plain_files() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("rill-completion-program");
    fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(directory.path().join("rill-completion-data"), b"").unwrap();
    let path = std::env::join_paths([directory.path(), directory.path()]).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["--internal-complete", "executable", "rill-completion-"])
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"xrill-completion-program\0");
}
