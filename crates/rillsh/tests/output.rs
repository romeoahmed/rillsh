//! Backpressure must not trap the coordinator in an unabortable output thread.
use std::{
    io,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn interrupt_reaps_a_writer_without_draining_its_output() -> io::Result<()> {
    use rustix::{
        event::{PollFd, PollFlags, poll},
        process::{Pid, Signal, kill_process},
    };
    let directory = tempfile::tempdir()?;
    std::fs::write(
        directory.path().join("payload"),
        vec![b'x'; 8 * 1024 * 1024],
    )?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args([
            "--color=never",
            "-c",
            "write_bytes (encode_utf8 (read_text \"payload\"))",
        ])
        .current_dir(directory.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child.stdout.take().unwrap();
    let mut ready = [PollFd::new(&output, PollFlags::IN)];
    let observed = poll(
        &mut ready,
        Some(&Duration::from_secs(10).try_into().unwrap()),
    )?;
    if observed == 0 {
        child.kill()?;
        child.wait()?;
        panic!("the write did not start");
    }
    kill_process(
        Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap(),
        Signal::INT,
    )?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // Keep the pipe open and unread until the assertion's observation is complete.
    drop(output);
    if status.is_none() {
        child.kill()?;
        child.wait()?;
    }
    assert_eq!(
        status
            .expect("cancellation waited for the pipe reader")
            .code(),
        Some(130)
    );
    Ok(())
}

#[test]
fn many_acknowledged_writes_preserve_exact_bytes_and_external_order() -> io::Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_rillsh"))
        .args(["-c", r#"each { n => do { write_bytes (encode_utf8 (string n)); write_bytes (encode_utf8 "\u{0}") } } [0, 1, 2, 3]; ^printf end"#])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"0\x001\x002\x003\x00end");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn an_unused_output_helper_closes_without_releasing_its_gate() -> io::Result<()> {
    let mut writer = rill_system::writer::Writer::prepare(
        std::path::Path::new(env!("CARGO_BIN_EXE_rillsh")),
        &rill_system::job::Snapshot::current()?,
        false,
    )
    .await?;
    let result = tokio::time::timeout(Duration::from_secs(10), writer.finish()).await;
    if result.is_err() {
        writer.cancel().await?;
    }
    result.expect("unused output helper waited forever at its gate")?;
    Ok(())
}
