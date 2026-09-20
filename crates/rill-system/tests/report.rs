//! Status policy must not turn a real downstream failure into successful cutoff.
use rill_system::{
    job::Termination,
    plan::{Plan, Stage},
    report,
};
use rustix::process::Signal;

#[test]
fn cleanup_provenance_and_connected_pipe_policy_preserve_failure_order() {
    let plan = Plan {
        stages: vec![Stage::default(), Stage::default()],
    };
    for (terminations, cleanup, expected) in [
        (
            [
                Termination::Signaled(Signal::PIPE.as_raw()),
                Termination::Exited(0),
            ],
            [false, false],
            None,
        ),
        (
            [
                Termination::Signaled(Signal::PIPE.as_raw()),
                Termination::Exited(7),
            ],
            [false, false],
            Some(1),
        ),
        (
            [
                Termination::Signaled(Signal::TERM.as_raw()),
                Termination::Exited(0),
            ],
            [true, false],
            None,
        ),
        (
            [
                Termination::Signaled(Signal::TERM.as_raw()),
                Termination::Exited(0),
            ],
            [false, false],
            Some(0),
        ),
        (
            [
                Termination::Exited(7),
                Termination::Signaled(Signal::TERM.as_raw()),
            ],
            [false, true],
            Some(0),
        ),
    ] {
        assert_eq!(
            report::classify_cutoff(&plan, &terminations, |i| cleanup[i]).failure,
            expected
        );
    }
}
