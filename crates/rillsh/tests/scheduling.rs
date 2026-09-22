//! Real process waits complement deterministic VM readiness tests.
use std::{
    io::{self, Read, Seek},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn merge_advances_a_peer_while_capture_waits_and_reaps_the_losing_job() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    for operation in ["capture", "run"] {
        let output = tempfile::tempfile()?;
        let source = r#"do {
  let blocked = items [()] |> map { () => capture plan { ^sh -c "touch started; sleep 30" } }
  let ready = stream plan { ^sh -c "while test ! -e started; do sleep 0.01; done; printf ready" }
  merge [blocked, ready] |> take 1 |> each write_bytes
}"#
        .replace("capture job", &format!("{operation} job"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_rillsh"))
            .args(["--color=never", "-c", &source])
            .current_dir(directory.path())
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let completed = loop {
            if child.try_wait()?.is_some() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if !completed {
            let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
            rustix::process::kill_process(pid, rustix::process::Signal::INT)?;
        }
        let result = child.wait_with_output()?;
        assert!(completed, "a pending capture prevented peer progress");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(directory.path().join("started").exists());
        let mut output = output;
        output.rewind()?;
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes)?;
        assert_eq!(bytes, b"ready");
        std::fs::remove_file(directory.path().join("started"))?;
    }
    Ok(())
}
