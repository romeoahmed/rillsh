//! Public invocation, process results, and ordered effects in isolated shell processes.
use std::{
    io,
    process::{Command, Output},
};
fn evaluate(source: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["--color=never", "-c", source])
        .output()
        .unwrap()
}
fn success(source: &str, stdout: &[u8]) {
    let output = evaluate(source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, stdout);
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn language_output_precedes_external_output() {
    success(
        "write_bytes (to_json (map { n => n * 2 } [1, 2, 3])); ^printf hello | ^cat",
        b"[2,4,6]hello",
    );
}

#[test]
fn merged_waiters_share_a_job_without_owning_or_reaping_it_twice() {
    success(
        r"do {
  let worker = start plan { ^sleep 0.05 }
  let reports = merge [items [worker] |> map wait, items [worker] |> map wait] |> collect
  print (string (length reports))
  wait worker
}",
        b"2\n",
    );
    success(
        r#"do {
  let worker = start plan { ^sleep 30 }
  merge [items [worker] |> map wait, items [worker] |> map { handle => do { cancel handle; () } }] |> collect
  wait worker
  print "joined"
}"#,
        b"joined\n",
    );
}
#[test]
fn capture_preserves_bytes_and_records_the_selected_failure() {
    success(
        r#"let result = capture (plan { ^sh -c "printf 'a\\000b'; printf error >&2; exit 7" }); write_bytes result.stdout; print (string (match result.report.failure of { Option.Some {value} => value, Option.None => -1 }))"#,
        b"a\0b0\n",
    );
    assert_eq!(evaluate("^sh -c \"exit 7\"").status.code(), Some(7));
    success("run (accept_exit [7, 7] (plan { ^sh -c \"exit 7\" }))", b"");
}
#[test]
fn native_environment_is_snapshotted_at_each_launch() {
    success(
        r#"let pipeline = plan { ^sh -c "printf %s \"$RILL_VALUE\"" }; set_env "RILL_VALUE" "first"; run pipeline; set_env "RILL_VALUE" "second"; run pipeline; run (with_env {RILL_VALUE: "override"} pipeline)"#,
        b"firstsecondoverride",
    );
}

#[test]
fn external_commands_do_not_inherit_unrelated_descriptors() -> io::Result<()> {
    use rustix::io::{FdFlags, fcntl_setfd};
    use std::os::{fd::AsRawFd, unix::process::CommandExt};

    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), b"private descriptor")?;
    for through_shell in [false, true] {
        let inherited = file.reopen()?;
        let path = format!("/dev/fd/{}", inherited.as_raw_fd());
        let mut command = if through_shell {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rillsh"));
            command.args(["-c", &format!("^/bin/cat {path}")]);
            command
        } else {
            let mut command = Command::new("/bin/cat");
            command.arg(path);
            command
        };
        // SAFETY: the owned descriptor stays open; fcntl is async-signal-safe and allocates nothing.
        // Only this child inherits it, leaving other concurrently running tests unaffected.
        unsafe {
            command.pre_exec(move || {
                fcntl_setfd(&inherited, FdFlags::empty())?;
                Ok(())
            });
        }
        let output = command.output()?;
        if through_shell {
            assert!(
                !output.status.success(),
                "an unrelated descriptor reached the target"
            );
            assert!(output.stdout.is_empty());
        } else {
            // Prove the fixture actually passes a readable descriptor across exec.
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, b"private descriptor");
        }
    }
    Ok(())
}
#[test]
fn capture_limit_failure_is_catchable_after_cleanup() {
    success(
        r#"let outcome = attempt { () => process.capture_with {max_bytes: 1} (plan { ^printf ab }) }; print (match outcome of { Result.Err {error} => error.kind, Result.Ok {value: _} => "unexpected" })"#,
        b"LimitExceeded\n",
    );
}
#[test]
fn arguments_and_redirects_preserve_native_bytes_and_order() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let script = directory.path().join("main.rill");
    std::fs::write(
        &script,
        "each write_bytes (args ()); ^sh -c \"printf out; printf err >&2\" > output 2>&1",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .arg(&script)
        .args(["", "hello world", "--flag"])
        .current_dir(directory.path())
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"hello world--flag");
    assert_eq!(std::fs::read(directory.path().join("output"))?, b"outerr");
    Ok(())
}
#[test]
fn scope_escape_and_duplicate_consumption_remain_language_errors() {
    for source in [
        "let source = items [1, 2]",
        "do { let source = items [1]; { () => collect source } }",
    ] {
        let output = evaluate(source);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("ResourceEscape"));
    }
    success(
        r#"print (string (do { let source = items [1, 2]; let alias = source; let output = take 1 source; let result = collect output; match attempt { () => collect alias } of { Result.Err {error} => error.kind == "StreamConsumed", Result.Ok {value: _} => false } }))"#,
        b"true\n",
    );
}

#[test]
fn directory_stream_uses_its_open_capability_after_rename() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::create_dir(directory.path().join("data"))?;
    std::fs::write(directory.path().join("data/a"), b"abc")?;
    std::fs::write(directory.path().join("data/.hidden"), b"x")?;
    std::os::unix::fs::symlink("a", directory.path().join("data/link"))?;
    let source = r#"
      do {
        let source = files "data"
        ^mv data moved
        source
          |> map { entry => {name: display_path entry.name, kind: entry.kind} }
          |> sort_by { entry => entry.name }
          |> to_json
          |> write_bytes
      }
    "#;
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["-c", source])
        .current_dir(directory.path())
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, br#"[{"name":".hidden","kind":"file"},{"name":"a","kind":"file"},{"name":"link","kind":"symlink"}]"#);
    Ok(())
}

#[test]
fn script_imports_resolve_from_the_script_directory() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::write(
        directory.path().join("module.rill"),
        "let answer = 42; export {answer}",
    )?;
    let script = directory.path().join("main.rill");
    std::fs::write(
        &script,
        "import \"./module.rill\" as module; print (string module.answer)",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .arg(script)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
    Ok(())
}

#[test]
fn process_streams_check_status_after_eof_and_close_on_cutoff() {
    success(
        r#"stream (plan { ^printf "%s\n" "3" "1" "2" }) |> lines |> map parse_int |> sort_by identity |> to_json |> write_bytes"#,
        b"[1,2,3]",
    );
    success(
        r#"print (match attempt { () => stream (plan { ^sh -c "printf x; exit 7" }) |> collect_bytes } of { Result.Err {error} => error.kind, _ => "unexpected" })"#,
        b"ProcessError\n",
    );
    success(
        "stream (plan { ^yes }) |> lines |> take 1 |> collect |> to_json |> write_bytes",
        br#"["y"]"#,
    );
    success(
        r#"stream (plan { ^sleep 10 }) |> take 0 |> close; print "closed""#,
        b"closed\n",
    );
}

#[test]
fn stdin_lease_is_exclusive_and_released_by_cutoff() -> io::Result<()> {
    use std::io::Write;
    use std::process::Stdio;
    let source = r#"
      do {
        let input = stdin ()
        print (match attempt { () => stdin () } of {
          Result.Err {error} => error.kind,
          _ => "unexpected"
        })
        print (match attempt { () => run (plan { ^cat }) } of {
          Result.Err {error} => error.kind,
          _ => "unexpected"
        })
        input |> lines |> take 1 |> collect |> to_json |> write_bytes
      }
      stdin () |> close
    "#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["-c", source])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(b"first\nsecond\n")?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"ResourceBusy\nResourceBusy\n[\"first\"]");
    Ok(())
}

#[test]
fn background_handles_retain_reports_and_use_explicit_acknowledgment() {
    success(
        r#"
      let worker = start (plan { ^sh -c "exit 7" })
      let report = wait worker
      print (string (report == wait worker))
      print (match attempt { () => check report } of {
        Result.Err {error} => error.kind,
        _ => "unexpected"
      })
      print (string (length (jobs ())))
    "#,
        b"true\nProcessError\n1\n",
    );
    success(
        "let worker = start (plan { ^sleep 30 }); cancel worker; cancel worker",
        b"",
    );
    success(
        r#"let worker = start (plan { ^sleep 30 }); cancel worker; print (match attempt { () => check (wait worker) } of { Result.Err {error} => error.kind, _ => "unexpected" })"#,
        b"ProcessError\n",
    );
    let output = evaluate("start (plan { ^sleep 30 }); ()");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unacknowledged"));
    success("let worker = start (plan { ^true }); fg worker", b"");
}

#[test]
fn stopped_background_jobs_can_continue_without_running_language_callbacks() {
    success(
        r#"
      let worker = start (plan { ^sh -c "kill -STOP $$; printf done" })
      print (match attempt { () => wait worker } of {
        Result.Err {error} => error.kind,
        _ => "unexpected"
      })
      bg worker
      check (wait worker)
    "#,
        b"JobStopped\ndone",
    );
}

#[test]
fn merge_makes_progress_while_another_input_waits_for_io() -> io::Result<()> {
    use std::{io::Read, process::Stdio, time::Duration};
    let mut child = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args([
            "-c",
            r#"
          do {
            let blocked = stdin () |> lines
            let ready = items ["ready"]
            merge [blocked, ready] |> take 1 |> each print
          }
        "#,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Keep the write end open: the first source must stay pending, not reach EOF.
    let input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output.read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });
    let result = receiver.recv_timeout(Duration::from_secs(10));
    if result.is_err() {
        child.kill()?;
    }
    drop(input);
    let status = child.wait()?;
    reader.join().unwrap();
    assert_eq!(
        result.expect("ready input was blocked behind stdin")?,
        b"ready\n"
    );
    assert!(status.success());
    Ok(())
}

#[test]
fn nested_merge_drains_every_source_and_checks_process_completion() {
    success(
        r#"merge [stream (plan { ^printf "1\n2\n" }) |> lines, merge [stream (plan { ^printf "3\n4\n" }) |> lines, items []]] |> map parse_int |> sort_by identity |> to_json |> write_bytes"#,
        b"[1,2,3,4]",
    );
    success(
        r#"print (match attempt { () => merge [stream (plan { ^sh -c "printf x; exit 7" }), chunks (encode_utf8 "y")] |> collect_bytes } of { Result.Err {error} => error.kind, _ => "unexpected" })"#,
        b"ProcessError\n",
    );
}

#[test]
fn through_pumps_both_directions_and_rejects_conflicting_redirects() {
    success(
        r#"chunks (encode_utf8 "hello") |> through (plan { ^cat | ^cat }) |> write_stdout"#,
        b"hello",
    );
    success(
        r#"range 0 10000 |> map { _ => encode_utf8 "hello\n" } |> through (plan { ^cat }) |> lines |> collect |> length |> string |> print"#,
        b"10000\n",
    );
    success(
        r#"range 0 4096 |> map { _ => encode_utf8 "abcdefghijklmnop\n" } |> through (plan { ^sh -c "dd if=/dev/zero bs=65536 count=4 2>/dev/null; cat" }) |> collect_bytes |> text.byte_length |> string |> print"#,
        b"331776\n",
    );
    success(
        "stream (plan { ^yes }) |> through (plan { ^head -n 1 }) |> write_stdout",
        b"y\n",
    );
    success(
        r#"do { let input = chunks (encode_utf8 "unchanged"); attempt { () => through (plan { ^cat > /dev/null }) input }; input |> write_stdout }"#,
        b"unchanged",
    );
    success(
        r#"print (match attempt { () => items [42] |> through (plan { ^cat }) |> collect_bytes } of { Result.Err {error} => error.kind, _ => "unexpected" })"#,
        b"TypeError\n",
    );
    success(
        r#"print (match attempt { () => chunks (bytes []) |> through (plan { ^sh -c "exit 7" }) |> collect_bytes } of { Result.Err {error} => error.kind, _ => "unexpected" })"#,
        b"ProcessError\n",
    );
}

#[test]
fn through_early_completion_closes_a_pending_upstream_read() -> io::Result<()> {
    use std::{io::Read, process::Stdio, time::Duration};
    let mut child = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args([
            "-c",
            "stdin () |> through (plan { ^printf ready }) |> write_stdout",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = sender.send(output.read_to_end(&mut bytes).map(|_| bytes));
    });
    let result = receiver.recv_timeout(Duration::from_secs(10));
    if result.is_err() {
        child.kill()?;
    }
    drop(input);
    let status = child.wait()?;
    reader.join().unwrap();
    assert_eq!(
        result.expect("completed job waited for upstream input")?,
        b"ready"
    );
    assert!(status.success());
    Ok(())
}

#[test]
fn session_filesystem_effects_follow_the_directory_capability_after_rename() -> io::Result<()> {
    let root = tempfile::tempdir()?;
    let original = root.path().join("original");
    std::fs::create_dir_all(original.join("child"))?;
    std::fs::write(original.join("value.txt"), "retained")?;
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .current_dir(&original)
        .args([
            "-c",
            r#"
          ^mv ../original ../renamed
          print (read_text "value.txt")
          print (string (glob "*.txt" == [path "value.txt"]))
          let previous = pwd ()
          let pipeline = with_cwd "child" (plan { ^pwd })
          cd "child"
          print (string (get_env "PWD" == some (encode_utf8 (display_path (pwd ())))))
          print (string (get_env "OLDPWD" == some (encode_utf8 (display_path previous))))
          print (string (ends_with "/renamed/child\n" (decode_utf8 (capture pipeline).stdout)))
        "#,
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"retained\ntrue\ntrue\ntrue\ntrue\n");
    Ok(())
}

#[test]
fn uncaught_errors_show_context_notes() {
    let output = evaluate(
        r#"raise (Error {kind: "Example", message: "operation failed", span: null, notes: ["additional context"], exit_status: null, details: {}})"#,
    );
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("Example"));
    assert!(diagnostic.contains("note: additional context"));
}

#[test]
fn cutoff_preserves_a_nonzero_exit_from_the_producers_termination_handler() {
    let output = evaluate(
        r#"stream (plan { ^sh -c 'trap "exit 7" TERM; printf x; while :; do :; done' }) |> take 1 |> collect_bytes |> write_bytes"#,
    );
    assert_eq!(output.status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ProcessError"));
    assert!(output.stdout.is_empty());
}

#[test]
fn resource_backed_producer_reads_its_owned_directory_and_releases_before_publication()
-> io::Result<()> {
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("entry"), b"payload")?;
    let source = format!(
        r#"
let values = seq.produce {{
  acquire: {{ () => do {{ print "acquire"; files {:?} }} }},
  step: {{ input => match input of {{
    () => Option.None,
    source => some [source |> collect |> map {{ entry => entry.size }} |> sum, ()]
  }} }},
  release: {{ state reason => print (match reason of {{
    seq.CloseReason.Exhausted => "release exhausted",
    _ => "unexpected"
  }}) }}
}} |> collect
print (string (sum values))
"#,
        directory.path().to_str().unwrap()
    );
    success(&source, b"acquire\nrelease exhausted\n7\n");
    let cutoff = format!(
        r#"
seq.produce {{
  acquire: {{ () => do {{ print "acquire"; files {:?} }} }},
  step: {{ input => some [42, input] }},
  release: {{ input reason => do {{
    close input
    print (match reason of {{ seq.CloseReason.Cutoff => "release cutoff", _ => "unexpected" }})
  }} }}
}} |> take 1 |> collect |> sum |> string |> print
"#,
        directory.path().to_str().unwrap()
    );
    success(&cutoff, b"acquire\nrelease cutoff\n42\n");
    Ok(())
}

#[test]
fn producer_release_runs_before_automatic_child_cleanup_and_error_recovery() {
    success(
        r#"
let result = attempt { () => seq.produce {
  acquire: { () => stream (plan { ^sleep 30 }) },
  step: { state => raise (error "StepFailure" "primary") },
  release: { state reason => do {
    print (match reason of { seq.CloseReason.Failed {error} => error.kind, _ => "unexpected" })
    close state
    print "release completed"
  } }
} |> collect }
print (match result of { Result.Err {error} => error.kind, _ => "unexpected" })
"#,
        b"StepFailure\nrelease completed\nStepFailure\n",
    );
}

#[test]
fn exit_finalizes_producers_and_language_error_names_do_not_control_the_session() {
    let output = evaluate(
        r#"
seq.produce {
  acquire: { () => 0 },
  step: { state => exit 3 },
  release: { state reason => print (match reason of { seq.CloseReason.Closed => "released", _ => "unexpected" }) }
} |> collect
"#,
    );
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout, b"released\n");
    let failed = evaluate(
        r#"
seq.produce {
  acquire: { () => 0 },
  step: { state => raise (error "StepFailure" "original failure") },
  release: { state reason => exit 3 }
} |> collect
"#,
    );
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("StepFailure"));
    for name in ["SessionExit", "Interrupted"] {
        let output = evaluate(&format!(
            "raise (error {name:?} \"ordinary language error\")"
        ));
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("ordinary language error"));
    }
}

#[test]
fn failed_acquisition_and_release_close_resources_created_inside_callbacks() {
    success(
        r#"
let acquisition = attempt { () => seq.produce {
  acquire: { () => do {
    let input = stream (plan { ^sleep 30 })
    raise (error "AcquireFailure" "before returning state")
  } },
  step: identity,
  release: { state reason => print "unexpected release" }
} |> collect }
print (match acquisition of { Result.Err {error} => error.kind, _ => "unexpected" })
let releasing = attempt { () => seq.produce {
  acquire: { () => 0 },
  step: { state => Option.None },
  release: { state reason => do {
    let input = stream (plan { ^sleep 30 })
    raise (error "ReleaseFailure" "after opening a resource")
  } }
} |> collect }
print (match releasing of { Result.Err {error} => error.kind, _ => "unexpected" })
print "recovered"
"#,
        b"AcquireFailure\nReleaseFailure\nrecovered\n",
    );
}

#[test]
fn helper_redirections_preserve_snapshot_order_append_and_native_path_bytes() -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let directory = tempfile::tempdir()?;
    let native = std::ffi::OsStr::from_bytes(b"--\t-output");
    let source = r#"
let destination = path (bytes [45, 45, 9, 45, 111, 117, 116, 112, 117, 116])
let separate = capture (plan { ^sh -c "printf out; printf err >&2" 2>&1 > $(destination) })
write_bytes separate.stdout
let combined = capture (plan { ^sh -c "printf more; printf error >&2" >> $(destination) 2>&1 })
write_bytes combined.stdout
let copied = capture (plan { ^cat < $(destination) })
write_bytes copied.stdout
^printf "%s" $(bytes [255])
"#;
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .current_dir(directory.path())
        .args(["-c", source])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"erroutmoreerror\xff");
    assert!(output.stderr.is_empty());
    assert_eq!(
        std::fs::read(directory.path().join(native))?,
        b"outmoreerror"
    );
    Ok(())
}

#[test]
fn glob_returns_literal_paths_without_interpreting_file_names() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let base = directory.path().join("[literal]*");
    std::fs::create_dir(&base)?;
    let mut names = ["a $(not-code)", "line\nname", "[x]", "plain", "quote\"name"];
    for name in names.iter().copied().chain([".hidden"]) {
        std::fs::write(base.join(name), [])?;
    }
    names.sort_unstable();
    let expected = names
        .iter()
        .map(|name| format!("path {name:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        r#"
print (string (glob "*" == [{expected}]))
print (string (glob "\\[x\\]" == [path "[x]"]))
print (string (glob "missing*" == [] and glob "" == []))
"#
    );
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .current_dir(base)
        .args(["-c", &source])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"true\ntrue\ntrue\n");
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn diagnostic_color_obeys_destination_and_explicit_override() {
    for (choice, colored) in [("auto", false), ("always", true), ("never", false)] {
        let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
            .args(["--color", choice, "-c", "let ="])
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr.contains(&0x1b), colored);
    }
}

#[test]
fn materialization_accounts_for_owned_job_plans() {
    let payload = "x".repeat(8192);
    let output = evaluate(&format!(
        "items [plan {{ ^echo '{payload}' }}] |> seq.collect_with {{max_bytes: 1024}}"
    ));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("LimitExceeded"));
    assert!(output.stdout.is_empty());
}
