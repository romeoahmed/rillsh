//! Exit policy preserves actual terminations while classifying connected-pipe cutoff.
use crate::{
    job::{Termination, is_sigpipe},
    plan::{Plan, Redirect},
};

pub struct Policy {
    pub cutoff: Vec<bool>,
    pub failure: Option<usize>,
}
#[must_use]
pub fn classify(plan: &Plan, terminations: &[Termination]) -> Policy {
    classify_cutoff(plan, terminations, |_| false)
}
/// Classify a finalized stream, preserving failures even during explicit cutoff.
/// The caller supplies only terminations covered by its recorded cleanup provenance.
pub fn classify_cutoff(
    plan: &Plan,
    terminations: &[Termination],
    expected: impl Fn(usize) -> bool,
) -> Policy {
    let mut policy = Policy {
        cutoff: vec![false; plan.stages.len()],
        failure: None,
    };
    for (index, (stage, termination)) in plan.stages.iter().zip(terminations).enumerate().rev() {
        let accepted = matches!(termination, Termination::Exited(code) if u8::try_from(*code).is_ok_and(|code| stage.accepted_codes.contains(&code)));
        let connected = index + 1 < plan.stages.len()
            && !stage.redirects.iter().any(|r| {
                matches!(
                    r,
                    Redirect::Write {
                        stream: crate::plan::Output::Stdout,
                        ..
                    }
                )
            })
            && !plan.stages[index + 1]
                .redirects
                .iter()
                .any(|r| matches!(r, Redirect::Read(_)));
        policy.cutoff[index] = policy.failure.is_none()
            && (expected(index) || (connected && is_sigpipe(*termination)));
        if !accepted && !policy.cutoff[index] && policy.failure.is_none() {
            policy.failure = Some(index);
        }
    }
    policy
}
#[must_use]
pub fn exit_code(termination: Termination) -> u8 {
    match termination {
        Termination::Exited(code) => u8::try_from(code).unwrap_or(1).max(1),
        Termination::Signaled(signal) => {
            u8::try_from(signal.saturating_add(128).clamp(1, 255)).unwrap_or(1)
        }
    }
}
