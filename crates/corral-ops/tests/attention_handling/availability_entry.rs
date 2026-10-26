use super::*;
use crate::attention::handling_tests::fixture::{Fx, at};

#[test]
fn real_availability_executor_entry_notifies_once_and_covers_later_owner_level() {
    let fx = Fx::new();
    let task = fx.questions(0);
    let run = task.current_run().unwrap();
    assert!(
        needs_me(&fx.ctx, &task, run, "fallback unavailable")
            .unwrap()
            .is_some()
    );
    let saved = fx.ctx.store.get(&task.id).unwrap();
    assert!(
        needs_me(&fx.ctx, &saved, run, "fallback unavailable")
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
    assert_eq!(notices[0].detail["producer"], "availability_exhausted");
}
