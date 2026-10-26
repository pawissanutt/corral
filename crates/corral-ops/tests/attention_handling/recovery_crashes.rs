use super::*;
use corral_core::attention::{AttentionFact, AttentionHandling};

fn fill_pending(fx: &Fx, key: &ConditionKey, count: u32) {
    let id = key.0.split('/').next().unwrap();
    fx.ctx
        .store
        .update_attention(id, |ledger| {
            for revision in 1..=count {
                let handling = AttentionHandling {
                    revision,
                    request: format!("pending-{revision}"),
                    note: "Investigating".into(),
                    at: at(i64::from(revision)),
                    next_check: at(60 + i64::from(revision)),
                    by: Responsible::Pm,
                };
                let record = ledger.records.get_mut(key).unwrap();
                record.first_handled_at.get_or_insert(handling.at);
                record.handling = Some(handling.clone());
                ledger.outbox.push(AttentionFact {
                    key: key.clone(),
                    fact_id: format!("{key}/attention_handled/{revision}"),
                    kind: "attention_handled".into(),
                    detail: json!({"key": key, "handling": handling}),
                });
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn wake_carriers_survive_capacity_reclaim_and_another_transition_crash() {
    // Given an unpublished opening plus three unpublished handling facts at capacity.
    let fx = Fx::new();
    let task = fx.stalled(None);
    let key = ConditionKey(key(&fx.ctx, &task));
    CRASH.with(|crash| crash.set(Some(CrashPoint::AfterTransition)));
    assert!(reconcile(&fx.ctx, at(0), Some(true)).is_err());
    fill_pending(&fx, &key, 3);
    assert_eq!(
        fx.ctx.store.get(&task.id).unwrap().attention.outbox.len(),
        4
    );
    // When reclaim succeeds but the subsequent level transaction stops before publication.
    CRASH.with(|crash| crash.set(Some(CrashPoint::AfterTransition)));
    assert!(reconcile(&fx.ctx, at(1800), Some(true)).is_err());
    let stopped = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(stopped.attention.outbox.len(), 3);
    assert_eq!(fx.logical("attention_handled").len(), 3);
    assert!(fx.logical("attention_due").is_empty());
    assert_eq!(fx.calls(), 0);
    // Then restart retains both wake carriers and emits one complete project digest.
    reconcile(&fx.reopen(), at(1801), Some(true)).unwrap();
    reconcile(&fx.reopen(), at(1802), Some(true)).unwrap();
    let events = fx.ctx.store.events().unwrap();
    let due: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "attention_due")
        .collect();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].detail["count"], 2);
    assert_eq!(due[0].detail["tasks"], json!([task.id]));
    assert!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .outbox
            .is_empty()
    );
    // The recorded pre-send owner claim cannot be sent after this crash.
    assert_eq!(fx.calls(), 0);
}

#[test]
fn full_terminal_outbox_keeps_tombstone_through_publication_crashes() {
    for point in [CrashPoint::AfterAppend, CrashPoint::BeforeClear] {
        // Given a terminal task with four accepted unpublished handling revisions.
        let fx = Fx::new();
        let task = fx.stalled(None);
        reconcile(&fx.ctx, at(0), Some(true)).unwrap();
        let key = ConditionKey(key(&fx.ctx, &task));
        fill_pending(&fx, &key, 4);
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.status = Status::Done;
                Ok(())
            })
            .unwrap();
        // When publication stops before acknowledgement.
        CRASH.with(|crash| crash.set(Some(point)));
        assert!(reconcile(&fx.ctx, at(5), Some(true)).is_err());
        let stopped = fx.ctx.store.get(&task.id).unwrap();
        assert_eq!(stopped.attention.outbox.len(), 4);
        assert!(stopped.attention.records.contains_key(&key));
        assert!(fx.logical("attention_resolved").is_empty());
        // Then a fresh pass publishes every fact before pruning the terminal record.
        reconcile(&fx.reopen(), at(6), Some(true)).unwrap();
        let after = fx.ctx.store.get(&task.id).unwrap();
        assert!(after.attention.records.is_empty());
        assert!(after.attention.outbox.is_empty());
        assert_eq!(fx.logical("attention_handled").len(), 4);
        assert_eq!(fx.logical("attention_resolved").len(), 1);
    }
}

#[test]
fn all_creation_paths_replay_their_announcement_after_publication_crashes() {
    for path in ["attend", "overflow"] {
        for point in [
            CrashPoint::AfterTransition,
            CrashPoint::AfterAppend,
            CrashPoint::BeforeClear,
        ] {
            // Given a record whose nonstandard opening stops at each lifecycle boundary.
            let fx = Fx::new();
            let task = if path == "overflow" {
                fx.questions(33)
            } else {
                fx.stalled(None)
            };
            let key = (path == "attend").then(|| ConditionKey(key(&fx.ctx, &task)));
            CRASH.with(|crash| crash.set(Some(point)));
            let stopped = match &key {
                Some(key) => attend(&fx.ctx, &key.0, "Looking", 1800, "first", at(0)).map(|_| ()),
                None => reconcile(&fx.ctx, at(0), Some(true)),
            };
            assert!(stopped.is_err(), "{path} {point:?}");
            let key = key.unwrap_or_else(|| {
                fx.ctx
                    .store
                    .get(&task.id)
                    .unwrap()
                    .attention
                    .records
                    .into_iter()
                    .find(|(_, record)| record.kind == "ledger_overflow")
                    .unwrap()
                    .0
            });
            assert!(fx.logical("attention_due").is_empty());
            // When a new context recovers and runs an unchanged pass.
            reconcile(&fx.reopen(), at(1), Some(true)).unwrap();
            reconcile(&fx.reopen(), at(2), Some(true)).unwrap();
            // Then the opening has one announcement, not a delayed re-wake substitute.
            let after = fx.ctx.store.get(&task.id).unwrap();
            assert_eq!(
                after.attention.records[&key].levels.announced_at,
                Some(at(0))
            );
            assert_eq!(after.attention.records[&key].levels.rewoke_at, None);
            let events = fx.ctx.store.events().unwrap();
            let due: Vec<_> = events
                .iter()
                .filter(|e| e.kind == "attention_due")
                .collect();
            assert_eq!(due.len(), 1, "{path} {point:?}");
            assert_eq!(
                due[0].detail["count"],
                if path == "overflow" { 32 } else { 1 }
            );
            assert_eq!(fx.calls(), 0);
        }
    }
}

#[test]
fn attend_created_handling_pauses_only_its_own_later_levels() {
    // Given two conditions and a handling-created record on just the first.
    let fx = Fx::new();
    let task = fx.questions(2);
    let keys: Vec<_> = board(&fx.ctx, at(0))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .filter_map(|c| c.key)
        .collect();
    attend(&fx.ctx, &keys[0].0, "Looking", 1800, "first", at(1)).unwrap();
    // When both deadlines pass while the first remains handled.
    reconcile(&fx.ctx, at(2), Some(true)).unwrap();
    reconcile(&fx.ctx, at(1800), Some(true)).unwrap();
    // Then only the second progresses; the first's announcement was not suppressed.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(
        after.attention.records[&keys[0]].levels.announced_at,
        Some(at(1))
    );
    assert_eq!(after.attention.records[&keys[0]].levels.rewoke_at, None);
    assert_eq!(
        after.attention.records[&keys[1]].levels.rewoke_at,
        Some(at(1800))
    );
    assert_eq!(fx.calls(), 1);
}
