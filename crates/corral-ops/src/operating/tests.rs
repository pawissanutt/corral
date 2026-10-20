use super::*;

fn refusal(reason: &str) -> anyhow::Error {
    AuthorizationRefused(reason.into()).into()
}

#[test]
fn capacity_and_snapshot_refusals_are_transient() {
    for reason in [
        "submission precondition refused: cannot request changes for T-1: waiting: machine busy (load/core 1.40)",
        "submission precondition refused: cannot request changes for T-1: waiting for a slot (1/1 tasks)",
        "submission precondition refused: T-1 or dispatch configuration changed; retry with the current state",
    ] {
        assert!(transient_refusal(&refusal(reason)), "{reason}");
    }
}

#[test]
fn other_refusals_and_ordinary_errors_are_not_transient() {
    assert!(!transient_refusal(&refusal(
        "automatic Repair is not authorized for T-1: paused"
    )));
    assert!(!transient_refusal(&refusal(
        "submission precondition refused: run changed after submitting changes"
    )));
    // A plain error that merely mentions a marker is not a submission refusal.
    assert!(!transient_refusal(&anyhow::anyhow!(
        "agent pane is gone: waiting: machine busy"
    )));
    assert!(!transient_refusal(&anyhow::anyhow!("waiting for a slot")));
}
