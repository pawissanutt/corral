//! Shared metadata-only condition fields; constructing these never performs source IO.
use corral_core::attention::{Progress, ResumeCheck};
use jiff::Timestamp;

pub(crate) fn progress(
    executor: &str,
    since: Option<Timestamp>,
    reason: String,
    liveness: &str,
) -> Progress {
    Progress {
        executor: executor.into(),
        since,
        reason,
        liveness: liveness.into(),
    }
}

pub(crate) fn check(
    kind: &str,
    target: &str,
    expected: Option<String>,
    satisfied: bool,
) -> ResumeCheck {
    ResumeCheck {
        kind: kind.into(),
        target: Some(target.into()),
        expected,
        description: format!("{kind}: {target}"),
        machine_checkable: true,
        satisfied: Some(satisfied),
    }
}
