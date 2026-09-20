//! Real helper launches exercise gates, transport, stage status and cleanup.
use rill_system::{
    job::{Job, Snapshot, State, Termination, read_chunk},
    plan::{Plan, Stage},
};
use std::{ffi::CString, os::fd::OwnedFd, path::Path};
use tokio::io::unix::AsyncFd;

fn plan(stages: &[&[&str]]) -> Plan {
    Plan {
        stages: stages
            .iter()
            .map(|args| Stage {
                argv: args.iter().map(|arg| CString::new(*arg).unwrap()).collect(),
                ..Stage::default()
            })
            .collect(),
    }
}
async fn drain(source: Option<AsyncFd<OwnedFd>>) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Some(source) = source {
        loop {
            let chunk = read_chunk(&source).await.unwrap();
            if chunk.is_empty() {
                break;
            }
            bytes.extend(chunk);
        }
    }
    bytes
}
#[tokio::test(flavor = "current_thread")]
async fn gated_pipeline_preserves_output_and_exit_status() {
    let plan = plan(&[&["printf", "%s", "hello world"], &["cat"]]);
    let mut job = Job::prepare(
        Path::new(env!("CARGO_BIN_EXE_rillsh")),
        &plan,
        &Snapshot::current().unwrap(),
        rill_system::job::LaunchMode::Capture,
    )
    .await
    .unwrap();
    job.commit().await.unwrap();
    let (stdout, stderr, state) = tokio::join!(
        drain(job.stdout.take()),
        drain(job.stderr.take()),
        job.wait()
    );
    assert_eq!(
        stdout,
        b"hello world",
        "stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    assert_eq!(state.unwrap(), State::Finished);
    assert_eq!(
        job.terminations(),
        [Some(Termination::Exited(0)), Some(Termination::Exited(0))]
    );
    job.reap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn launch_failure_is_distinct_from_target_exit_127() {
    let context = Snapshot::current().unwrap();
    let mut failure = Job::prepare(
        Path::new(env!("CARGO_BIN_EXE_rillsh")),
        &plan(&[&["/rill-test/missing-executable"]]),
        &context,
        rill_system::job::LaunchMode::Capture,
    )
    .await
    .unwrap();
    assert!(failure.commit().await.is_err());
    failure.cancel().await.unwrap();
    let mut exited = Job::prepare(
        Path::new(env!("CARGO_BIN_EXE_rillsh")),
        &plan(&[&["/bin/sh", "-c", "exit 127"]]),
        &context,
        rill_system::job::LaunchMode::Capture,
    )
    .await
    .unwrap();
    exited.commit().await.unwrap();
    assert_eq!(exited.wait().await.unwrap(), State::Finished);
    assert_eq!(exited.terminations(), [Some(Termination::Exited(127))]);
    exited.reap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn exec_target_receives_default_sigpipe_disposition() {
    let mut job = Job::prepare(
        Path::new(env!("CARGO_BIN_EXE_rillsh")),
        &plan(&[&["/bin/sh", "-c", "kill -PIPE $$; exit 99"]]),
        &Snapshot::current().unwrap(),
        rill_system::job::LaunchMode::Capture,
    )
    .await
    .unwrap();
    job.commit().await.unwrap();
    assert_eq!(job.wait().await.unwrap(), State::Finished);
    assert_eq!(
        job.terminations(),
        [Some(Termination::Signaled(
            rustix::process::Signal::PIPE.as_raw()
        ))]
    );
    job.reap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn setup_failure_keeps_every_target_behind_the_launch_barrier() {
    use rill_system::plan::Redirect;
    let directory = tempfile::tempdir().unwrap();
    let ran = directory.path().join("ran");
    let mut plan = plan(&[&["sh", "-c", "printf ran > \"$1\"", "stage"], &["cat"]]);
    plan.stages[0]
        .argv
        .push(CString::new(ran.as_os_str().as_encoded_bytes()).unwrap());
    plan.stages[1]
        .redirects
        .push(Redirect::Read(directory.path().join("missing")));
    let mut job = Job::prepare(
        Path::new(env!("CARGO_BIN_EXE_rillsh")),
        &plan,
        &Snapshot::current().unwrap(),
        rill_system::job::LaunchMode::Capture,
    )
    .await
    .unwrap();
    let error = job.commit().await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    job.cancel().await.unwrap();
    assert!(!ran.exists());
    assert!(job.terminations().iter().all(Option::is_some));
    assert!(job.group().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn glob_worker_keeps_directory_identity_across_rename_and_path_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("[base]*");
    let renamed = directory.path().join("renamed");
    std::fs::create_dir(&original).unwrap();
    std::fs::write(original.join("kept"), []).unwrap();
    let mut snapshot = Snapshot::current().unwrap();
    snapshot.set_cwd(&original).unwrap();
    let executable = env!("CARGO_BIN_EXE_rillsh");
    let mut job = Job::prepare(
        Path::new(executable),
        &plan(&[&[executable, "--internal-glob", "*"]]),
        &snapshot,
        rill_system::job::LaunchMode::Capture,
    )
    .await
    .unwrap();
    std::fs::rename(&original, &renamed).unwrap();
    std::fs::create_dir(&original).unwrap();
    std::fs::write(original.join("decoy"), []).unwrap();
    job.commit().await.unwrap();
    let (stdout, stderr, state) = tokio::join!(
        drain(job.stdout.take()),
        drain(job.stderr.take()),
        job.wait()
    );
    assert_eq!(state.unwrap(), State::Finished);
    assert_eq!(stdout, b"kept\0");
    assert!(stderr.is_empty());
    assert_eq!(job.terminations(), [Some(Termination::Exited(0))]);
    job.reap().unwrap();
}
