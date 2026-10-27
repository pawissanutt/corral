//! Raw push plus attention wakes: the single-question fixture count, then extra kinds and replays.
use crate::fixture::Fx;
use crate::support::{invariants, only_key, pm_live, stats, sync};

fn push(fx: &mut Fx, extra: &[&str]) {
    fx.ctx.store.config.pm.push_events = true;
    let kinds = &mut fx.ctx.store.config.pm.push_kinds;
    for kind in ["question_opened"].iter().chain(extra) {
        if !kinds.iter().any(|existing| existing == kind) {
            kinds.push((*kind).into());
        }
    }
    fx.ctx.store.save_config().unwrap();
    pm_live(fx);
}

#[test]
fn isolated_single_question_gets_one_raw_push_and_two_attention_levels() {
    // Given push enabled, a live PM and one non-blocking question.
    let mut fx = Fx::new();
    push(&mut fx, &[]);
    let task = fx.questions(1);
    // When passes run through the re-wake and owner bounds.
    for seconds in [0, 900, 1800, 2700] {
        sync(&fx, seconds);
    }
    // Then the enumerated fixture count is one question push plus two attention levels.
    let key = only_key(&fx, &task.id);
    let stats = stats(&fx, 2700);
    assert_eq!(
        stats.wakes.raw_push.by_kind.get("question_opened"),
        Some(&1)
    );
    assert_eq!(stats.wakes.raw_push.attributed.get(&key), Some(&1));
    assert_eq!(stats.conditions[&key].wakes, 2);
    assert_eq!(
        stats.wakes.raw_push.attributed[&key] + stats.conditions[&key].wakes,
        3
    );
    assert_eq!(stats.wakes.attention_due, 2);
    assert_eq!(stats.wakes.raw_push.by_kind.get("attention_due"), Some(&2));
    invariants(&stats);
}

#[test]
fn extra_raw_kinds_and_replayed_receipts_are_reported_without_raising_attention_bounds() {
    // Given push enabled for further unrelated kinds, with two tasks in one project.
    let mut fx = Fx::new();
    push(
        &mut fx,
        &["message_unobserved", "turn_ended_without_report"],
    );
    let question = fx.questions(1);
    let stalled = fx.stalled(None);
    for seconds in [0, 900, 1800] {
        sync(&fx, seconds);
    }
    // When every delivery receipt is replayed into the log.
    let receipts: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "pm_event_delivery")
        .collect();
    assert!(!receipts.is_empty());
    for receipt in &receipts {
        fx.ctx
            .store
            .log_event(
                receipt.task.as_deref(),
                &receipt.kind,
                receipt.detail.clone(),
            )
            .unwrap();
    }
    // Then raw pushes are deduplicated and reported apart from the attention bounds.
    let stats = stats(&fx, 1800);
    assert_eq!(stats.wakes.raw_push.replayed, receipts.len() as u64);
    assert_eq!(stats.wakes.raw_push.submitted, receipts.len() as u64);
    assert_eq!(stats.wakes.attention_due, stats.wakes.due_pairs);
    for id in [&question.id, &stalled.id] {
        assert!(stats.conditions[&only_key(&fx, id)].wakes <= 2);
    }
    invariants(&stats);
}
