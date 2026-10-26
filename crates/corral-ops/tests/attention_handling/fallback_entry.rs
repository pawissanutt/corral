use super::*;
use crate::attention::handling_tests::fixture::{Fx, at};

#[test]
fn real_legacy_fallback_entry_keeps_its_identity_without_ladder_resend() {
    let fx = Fx::new();
    let task = fx.stalled(None);
    let run = task.current_run().unwrap();
    assert!(
        escalate(&fx.ctx, &task, run, "fallback blocked", at(0))
            .unwrap()
            .is_some()
    );
    assert!(
        escalate(&fx.ctx, &task, run, "fallback blocked", at(1))
            .unwrap()
            .is_none()
    );
    crate::attention::reconcile(&fx.reopen(), at(0), Some(true)).unwrap();
    crate::attention::reconcile(&fx.reopen(), at(7200), Some(true)).unwrap();
    assert_eq!(fx.calls(), 1);
    let events = fx.ctx.store.events().unwrap();
    let notices: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "owner_notified")
        .collect();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].detail["producer"], "legacy_review_fallback");
}
