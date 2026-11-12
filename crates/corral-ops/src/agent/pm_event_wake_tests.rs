use super::*;
use crate::agent::BindTarget;
use crate::agent::test_support::{self as ts, Fx};
use corral_core::agent::pm_state;

fn bind(fx: &Fx) {
    let host = ts::bind(
        fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "s-pm",
    );
    ts::committed(&ts::hello(fx, &host));
    assert!(super::super::pm_wake::healthy(
        &pm_state::load_current(&fx.ctx.store).unwrap().unwrap()
    ));
}

fn inbox(fx: &Fx) -> Inbox<'_> {
    Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap()
}

fn event(fx: &Fx, task: Option<&str>, kind: &str, detail: serde_json::Value) -> Event {
    let mut reader = fx.ctx.store.events_since(None).unwrap();
    fx.ctx.store.log_event(task, kind, detail).unwrap();
    reader.next_event().unwrap().unwrap().0
}

#[test]
fn native_route_coalesces_and_checkpoints_even_when_paste_push_is_disabled() {
    let mut fx = ts::fx();
    fx.ctx.store.config.pm.push_events = false;
    fx.ctx.store.config.pm.push_kinds = vec!["done".into()];
    bind(&fx);
    // First entry checkpoints EOF: historical events must not become a first-binding flood.
    event(&fx, None, "done", json!({}));
    crate::pm_events::push_events(&fx.ctx, false).unwrap();
    assert!(inbox(&fx).messages().unwrap().is_empty());
    event(&fx, Some("T-001"), "done", json!({}));
    event(&fx, Some("T-002"), "done", json!({}));
    let last = event(&fx, Some("T-003"), "done", json!({}));
    crate::pm_events::push_events(&fx.ctx, false).unwrap();
    let messages = inbox(&fx).messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, "pm_event_wake");
    assert_eq!(messages[0].subject.as_ref().unwrap()["last_seq"], last.seq);
    assert_eq!(
        STATE.load(&fx.ctx.store, "state").unwrap().unwrap().seq,
        last.seq
    );
    crate::pm_events::push_events(&fx.ctx, false).unwrap();
    assert_eq!(inbox(&fx).messages().unwrap(), messages);
    // A missing host control endpoint did not hold the cursor or prevent a later wake.
    event(&fx, Some("T-004"), "done", json!({}));
    crate::pm_events::push_events(&fx.ctx, false).unwrap();
    assert_eq!(inbox(&fx).messages().unwrap().len(), 2);
}

#[test]
fn publish_replay_deduplicates_urgent_and_routine_publications() {
    let fx = ts::fx();
    let routine = event(&fx, Some("T-001"), "done", json!({}));
    let urgent = event(&fx, Some("T-002"), "review_escalated", json!({}));
    let events = [routine.clone(), urgent.clone()];
    assert_eq!(publish_pass(&inbox(&fx), &events).unwrap(), 2);
    let messages = inbox(&fx).messages().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].subject.as_ref().unwrap()["key"],
        format!("cond:T-002:escalated#{}", urgent.seq)
    );
    assert_eq!(
        messages[1].subject.as_ref().unwrap()["key"],
        format!("pmev:{}-{}", routine.seq, routine.seq)
    );
    // Crash after publication: no cursor has been saved, so exactly the same pass replays.
    assert!(STATE.load(&fx.ctx.store, "state").unwrap().is_none());
    assert_eq!(publish_pass(&inbox(&fx), &events).unwrap(), 0);
    assert_eq!(inbox(&fx).messages().unwrap(), messages);
}

#[test]
fn non_pi_disabled_push_skips_events_and_never_posts_to_the_inbox() {
    let mut fx = ts::fx();
    fx.ctx.store.config.pm.push_events = false;
    event(&fx, Some("T-001"), "done", json!({}));
    crate::pm_events::push_events(&fx.ctx, false).unwrap();
    assert!(inbox(&fx).messages().unwrap().is_empty());
    // Events skipped without a Pi binding are not replayed when it later binds.
    bind(&fx);
    crate::pm_events::push_events(&fx.ctx, false).unwrap();
    assert!(inbox(&fx).messages().unwrap().is_empty());
}

#[test]
fn selection_preserves_defaults_and_always_includes_attention_due() {
    let mut fx = ts::fx();
    let question_status = event(
        &fx,
        Some("T-001"),
        "needs_me",
        json!({"question_keys": ["T-001/R1/Q1"]}),
    );
    assert!(!selected(&fx.ctx, &question_status));
    fx.ctx.store.config.pm.push_kinds = vec!["needs_me".into()];
    assert!(selected(&fx.ctx, &question_status));
    assert_eq!(causes(&question_status), vec!["q:T-001/R1/Q1"]);
    fx.ctx.store.config.pm.push_kinds.clear();
    let attention = event(&fx, None, "attention_due", json!({}));
    assert!(selected(&fx.ctx, &attention));
    assert!(!selected(&fx.ctx, &question_status));
}

#[test]
fn digest_keeps_newest_per_cause_counts_repeats_and_bounds_large_events() {
    let fx = ts::fx();
    let first = event(&fx, Some("T-001"), "done", json!({"reason": "old"}));
    let second = event(&fx, Some("T-001"), "done", json!({"reason": "new"}));
    let huge = event(
        &fx,
        Some("T-002"),
        "done",
        json!({"reason": "界".repeat(super::super::delivery::INPUT_MAX_BYTES)}),
    );
    publish_pass(&inbox(&fx), &[first, second.clone(), huge]).unwrap();
    let message = inbox(&fx).messages().unwrap().remove(0);
    assert!(
        message
            .body
            .contains(&format!("{} (count: 2)", format_event_line(&second)))
    );
    assert!(message.body.contains("1 causes omitted"));
    assert!(message.body.len() < super::super::delivery::INPUT_MAX_BYTES);
    assert_eq!(
        message.subject.unwrap()["causes"].as_array().unwrap().len(),
        2
    );
}
