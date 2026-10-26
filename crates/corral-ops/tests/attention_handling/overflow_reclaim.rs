use corral_core::attention::{AttentionHandling, ConditionKey, Responsible};
use serde_json::json;

use super::fixture::{Fx, at};
use super::recovery::public_attend;

fn retained_resolutions(
    keep_recorded: usize,
    accepted: Option<&AttentionHandling>,
) -> (Fx, String, ConditionKey) {
    let fx = Fx::new();
    let task = fx.questions(33);
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    let saved = fx.ctx.store.get(&task.id).unwrap();
    let overflow = saved
        .attention
        .records
        .iter()
        .find(|(_, record)| record.kind == "ledger_overflow")
        .unwrap()
        .0
        .clone();
    if let Some(value) = accepted {
        assert_eq!(public_attend(&fx, &overflow, value).unwrap(), *value);
    }
    fx.ctx
        .store
        .update(&task.id, |task| {
            let id = task.id.clone();
            let questions = &mut task
                .current_run_mut()
                .unwrap()
                .agent_host
                .as_mut()
                .unwrap()
                .questions;
            let mut kept = 0;
            questions.retain_mut(|question| {
                if saved.attention.records.contains_key(&ConditionKey(format!(
                    "{id}/question_open/{}@{}",
                    question.key, question.revision
                ))) {
                    kept += 1;
                    kept <= keep_recorded
                } else {
                    question.revision = 2;
                    question.asked.at = at(1799);
                    true
                }
            });
            Ok(())
        })
        .unwrap();
    std::fs::write(fx.tmp.path().join("inventory-down"), "").unwrap();
    let log = fx.ctx.store.dir().join("events.jsonl");
    let backup = fx.ctx.store.dir().join("events.before-failure");
    std::fs::rename(&log, &backup).unwrap();
    std::fs::create_dir(&log).unwrap();
    let error = corral_ops::sync_at(&fx.reopen(), at(1799)).unwrap_err();
    let stopped = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(
        stopped
            .attention
            .records
            .values()
            .filter(|record| record.resolved_at.is_some())
            .count(),
        31 - keep_recorded
    );
    assert_eq!(
        stopped
            .attention
            .outbox
            .iter()
            .filter(|fact| fact.kind == "attention_resolved")
            .count(),
        31 - keep_recorded
    );
    assert_eq!(stopped.attention.overflow_count, Some(2));
    println!("PUBLICATION_FAILURE {error:#}");
    std::fs::remove_dir(&log).unwrap();
    std::fs::rename(&backup, &log).unwrap();
    (fx, task.id, overflow)
}

fn handling(request: &str) -> AttentionHandling {
    AttentionHandling {
        revision: 1,
        request: request.into(),
        note: "Investigating".into(),
        at: at(1800),
        next_check: at(3600),
        by: Responsible::Pm,
    }
}

#[test]
fn restart_reclaims_capacity_before_overflow_owner_level() {
    // Given a real append failure retaining 31 resolutions and two fresh questions.
    let (fx, id, overflow) = retained_resolutions(0, None);
    // When restarted sync publishes the resolutions at the overflow owner deadline.
    corral_ops::sync_at(&fx.reopen(), at(1800)).unwrap();
    // Then overflow resolves before any owner level, and only the fresh questions remain.
    let after = fx.ctx.store.get(&id).unwrap();
    assert_eq!(fx.calls(), 0);
    assert_eq!(after.attention.records.len(), 2);
    assert!(!after.attention.records.contains_key(&overflow));
    assert_eq!(after.attention.overflow_count, None);
    assert!(after.attention.outbox.is_empty());
    let events = fx.ctx.store.events().unwrap();
    assert!(
        !events
            .iter()
            .any(|event| event.kind == "notification_claimed")
    );
    assert!(!events.iter().any(|event| {
        event.kind == "attention_overdue"
            && event.detail["key"] == overflow.0
            && event.detail["level"] == "owner"
    }));
    let resolved = events
        .iter()
        .find(|event| event.kind == "attention_resolved" && event.detail["key"] == overflow.0)
        .unwrap();
    for field in [
        "kind",
        "responsible",
        "since",
        "since_source",
        "opened_at",
        "pm_live_at_open",
        "first_handled_at",
        "levels",
        "resolved_at",
    ] {
        assert!(resolved.detail.get(field).is_some(), "{field}");
    }
    assert_eq!(fx.logical("attention_resolved").len(), 32);
    // The retained re-wake carrier still gets its complete digest before acknowledgement.
    assert!(events.iter().any(|event| {
        event.kind == "attention_due"
            && event.detail["at"] == json!(at(1800))
            && event.detail["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["key"] == overflow.0)
    }));
    corral_ops::sync_at(&fx.reopen(), at(1801)).unwrap();
    assert_eq!(fx.calls(), 0);
    assert_eq!(fx.ctx.store.get(&id).unwrap().attention, after.attention);
}

#[test]
fn new_handling_refuses_overflow_made_stale_by_its_own_publication() {
    // Given the same durable failed-publication shape, with no accepted request.
    let (fx, id, overflow) = retained_resolutions(0, None);
    // When public attend itself frees the ordinary slots before handling acceptance.
    let error = public_attend(&fx, &overflow, &handling("reclaim")).unwrap_err();
    // Then it names current keys and creates no handling, while retaining the wake carrier.
    assert!(error.starts_with("stale_item:"), "{error}");
    assert!(error.contains("question_open"), "{error}");
    let board = corral_ops::attention::board(&fx.reopen(), at(1800)).unwrap();
    assert!(
        !board
            .tasks
            .iter()
            .find(|row| row.task == id)
            .unwrap()
            .assessment
            .conditions
            .iter()
            .any(|condition| condition.key.as_ref() == Some(&overflow))
    );
    let after = fx.ctx.store.get(&id).unwrap();
    assert!(after.attention.records[&overflow].handling.is_none());
    assert_eq!(fx.logical("attention_handled").len(), 0);
    assert!(
        after
            .attention
            .outbox
            .iter()
            .any(|fact| fact.detail["wake"] == true)
    );
    assert_eq!(fx.calls(), 0);
    // A later sync, rather than rejected handling, owns resolution and wake acknowledgement.
    corral_ops::sync_at(&fx.reopen(), at(1800)).unwrap();
    assert!(
        !fx.ctx
            .store
            .get(&id)
            .unwrap()
            .attention
            .records
            .contains_key(&overflow)
    );
    assert_eq!(fx.logical("attention_resolved").len(), 32);
}

#[test]
fn continuing_overflow_keeps_identity_and_owner_claim_after_partial_reclaim() {
    // Given only one pending resolution, so one of the two fresh questions still cannot fit.
    let (fx, id, overflow) = retained_resolutions(30, None);
    let before = fx.ctx.store.get(&id).unwrap().attention.records[&overflow].clone();
    // When reclaimed capacity still leaves a shortage at the owner deadline.
    corral_ops::sync_at(&fx.reopen(), at(1800)).unwrap();
    // Then the same episode advances once, without evicting any live record.
    let after = fx.ctx.store.get(&id).unwrap();
    assert_eq!(after.attention.records.len(), 32);
    assert_eq!(after.attention.overflow_count, Some(1));
    assert_eq!(after.attention.records[&overflow].since, before.since);
    assert_eq!(
        after.attention.records[&overflow].opened_at,
        before.opened_at
    );
    assert_eq!(
        after.attention.records[&overflow].levels.owner_claimed_at,
        Some(at(1800))
    );
    assert_eq!(fx.calls(), 1);
    let events = fx.ctx.store.events().unwrap();
    let claim = events
        .iter()
        .find(|event| event.kind == "notification_claimed")
        .unwrap();
    assert!(
        claim.detail["coverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|coverage| coverage["condition_key"] == overflow.0)
    );
    corral_ops::sync_at(&fx.reopen(), at(1801)).unwrap();
    assert_eq!(fx.calls(), 1);
}

#[test]
fn accepted_overflow_replay_returns_before_reclaim_or_publication() {
    // Given accepted overflow handling followed by the real failed resolution publication.
    let mut accepted = handling("already-accepted");
    accepted.at = at(1);
    accepted.next_check = at(61);
    let (fx, id, overflow) = retained_resolutions(0, Some(&accepted));
    let before = fx.ctx.store.get(&id).unwrap();
    let events = fx.ctx.store.events().unwrap();
    let mut replay = accepted.clone();
    replay.at = at(1800);
    replay.next_check = at(1860);
    // When the same request is replayed, including a conflicting-payload control.
    assert_eq!(public_attend(&fx, &overflow, &replay).unwrap(), accepted);
    replay.note.push('!');
    assert_eq!(
        public_attend(&fx, &overflow, &replay).unwrap_err(),
        "request_payload_mismatch"
    );
    // Then neither replay drains the pending resolutions nor mutates task or event bytes.
    assert_eq!(fx.ctx.store.get(&id).unwrap(), before);
    assert_eq!(fx.ctx.store.events().unwrap(), events);
    assert_eq!(fx.calls(), 0);
}
