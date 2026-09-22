//! File streams and plan composition preserve bytes, ordering and cleanup.
use std::process::Command;

fn run(directory: &std::path::Path, source: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .current_dir(directory)
        .args(["--color=never", "-c", source])
        .output()
        .unwrap()
}
fn success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn file_streams_round_trip_binary_data_and_append() {
    let directory = tempfile::tempdir().unwrap();
    let output = run(
        directory.path(),
        r#"
      chunks (bytes [0, 255, 13, 10]) |> write_file "data"
      chunks (bytes [42]) |> append_file "data"
      read_file "data" |> write_file "copy"
      read_bytes "copy" |> write_bytes
      eprint "notice"
    "#,
    );
    success(&output);
    assert_eq!(output.stdout, [0, 255, 13, 10, 42]);
    assert_eq!(output.stderr, b"notice\n");
    assert_eq!(
        std::fs::read(directory.path().join("copy")).unwrap(),
        output.stdout
    );
    let output = run(
        directory.path(),
        r#"fs.read_bytes_with {max_bytes: 2} "copy""#,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("LimitExceeded"));
}
#[test]
fn plan_redirects_are_ordered_and_environment_removal_is_local() {
    let directory = tempfile::tempdir().unwrap();
    let output = run(
        directory.path(),
        r#"
      set_env "RILL_PLAN_TEST" "present"
      let pipeline = command "sh" ["-c", "printf '%s' ${RILL_PLAN_TEST-absent}; printf err >&2"]
      pipeline |> without_env ["RILL_PLAN_TEST"] |> with_stdout "out" |> stderr_to_stdout |> run
      read_bytes "out" |> write_bytes
      match get_env "RILL_PLAN_TEST" of { Option.Some {value} => write_bytes value, _ => () }
    "#,
    );
    success(&output);
    assert_eq!(output.stdout, b"absenterrpresent");
    assert!(output.stderr.is_empty());
}
#[test]
fn caught_process_failure_retains_exit_status_and_stage() {
    let directory = tempfile::tempdir().unwrap();
    let output = run(
        directory.path(),
        r#"match attempt { () => run plan { ^sh -c "exit 7" } } of {
      Result.Err {error} => do { error.details.stage |> string |> print; raise error },
      Result.Ok {value} => value
    }"#,
    );
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"0\n");
}

#[test]
fn source_tools_never_execute_or_resolve_imports() {
    let directory = tempfile::tempdir().unwrap();
    let source = "import 'missing.rill' as missing\ndo {\nprint 'effect'\n}\n";
    for mode in ["--check", "--format"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
            .current_dir(directory.path())
            .args([mode, "-c", source])
            .output()
            .unwrap();
        success(&output);
        if mode == "--format" {
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                "import 'missing.rill' as missing\ndo {\n  print 'effect'\n}\n"
            );
        } else {
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn dynamic_file_streams_keep_opened_paths_and_cleanup_after_failure() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("first"), b"a").unwrap();
    std::fs::write(directory.path().join("second"), b"b").unwrap();
    std::fs::create_dir(directory.path().join("nested")).unwrap();
    let output = run(
        directory.path(),
        r#"do {
      let source = read_file "first"
      cd "nested"
      source |> write_stdout
    }
    cd ".."
    items ["first", "second"] |> flat_map read_file |> write_file "joined"
    read_bytes "joined" |> write_bytes
    match attempt { () => items [bytes [120], 42] |> write_file "partial" } of {
      Result.Err {error} => eprint error.kind,
      Result.Ok {value} => value
    }
    read_bytes "partial" |> write_bytes
    merge [read_file "first", read_file "second"] |> collect_bytes |> text.byte_length |> string |> print
    "#,
    );
    success(&output);
    assert_eq!(output.stdout, b"aabx2\n");
    assert_eq!(output.stderr, b"TypeError\n");
}
