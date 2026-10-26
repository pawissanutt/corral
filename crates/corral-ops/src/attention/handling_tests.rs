#![cfg(unix)]
use std::collections::BTreeSet;

use super::*;
use corral_core::attention::{AttentionLedger, MAX_ATTENTION_LEDGER_BYTES};
use corral_core::{Status, Task};
use handling::attend_authorized as attend;
use handling::{CRASH, CrashPoint, reconcile};
use serde_json::json;

#[path = "../../tests/attention_handling/fixture.rs"]
pub(crate) mod fixture;
use fixture::{Fx, at};

#[path = "../../tests/attention_handling/recovery_crashes.rs"]
mod recovery_crashes;

fn key(ctx: &Ctx, task: &Task) -> String {
    board(ctx, at(0))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find_map(|condition| condition.key.map(|key| key.0))
        .unwrap()
}

#[test]
fn announcements_are_quiet_until_another_condition_opens() {
    // Given one newly stalled review.
    let fx = Fx::new();
    let first = fx.stalled(Some("alpha"));
    // When repeated passes see no canonical change, then only the opening wakes.
    for _ in 0..11 {
        reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    }
    assert_eq!(fx.logical("attention_opened").len(), 1);
    assert_eq!(fx.logical("attention_due").len(), 1);
    let second = fx.stalled(Some("alpha"));
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    let due: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "attention_due")
        .collect();
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].detail["tasks"], json!([first.id]));
    assert_eq!(due[1].detail["tasks"], json!([second.id]));
}

#[test]
fn ladder_deadlines_and_restart_do_not_repeat_levels_or_transport() {
    // Given a PM condition with canonical time zero.
    let fx = Fx::new();
    let task = fx.stalled(None);
    // When both deadlines and twenty later passes run across a new Ctx.
    for seconds in [0, 900, 1800] {
        reconcile(&fx.ctx, at(seconds), Some(true)).unwrap();
    }
    let reopened = fx.reopen();
    for _ in 0..20 {
        reconcile(&reopened, at(7200), Some(true)).unwrap();
    }
    // Then announcement, re-wake, and owner escalation are each durable and unique.
    assert_eq!(fx.logical("attention_due").len(), 2);
    assert_eq!(fx.logical("attention_overdue").len(), 2);
    assert_eq!(fx.calls(), 1);
    assert_eq!(fx.logical("owner_notified").len(), 1);
    let after = reopened.store.get(&task.id).unwrap();
    let record = after.attention.records.values().next().unwrap();
    assert_eq!(record.levels.announced_at, Some(at(0)));
    assert_eq!(record.levels.rewoke_at, Some(at(900)));
    assert_eq!(record.levels.owner_claimed_at, Some(at(1800)));
}

#[test]
fn handling_replay_expiry_and_new_revision_never_resolve_or_authorize() {
    // Given a recorded condition and a handling note before re-wake.
    let fx = Fx::new();
    let task = fx.stalled(None);
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    let key = key(&fx.ctx, &task);
    let before = fx.ctx.store.get(&task.id).unwrap();
    let accepted = attend(&fx.ctx, &key, "Investigating", 1800, "request-1", at(100)).unwrap();
    // When the lost output is replayed and time advances through next_check.
    assert_eq!(
        attend(&fx.ctx, &key, "Investigating", 1800, "request-1", at(200)).unwrap(),
        accepted
    );
    reconcile(&fx.ctx, at(1899), Some(true)).unwrap();
    assert!(fx.logical("attention_overdue").is_empty());
    assert_eq!(fx.calls(), 0);
    reconcile(&fx.ctx, at(1900), Some(true)).unwrap();
    let second = attend(
        &fx.ctx,
        &key,
        "Waiting for evidence",
        1800,
        "request-2",
        at(1901),
    )
    .unwrap();
    // Then only handling changed; no answer, status, review or authorization changed.
    assert_eq!(second.revision, 2);
    assert_eq!(
        attend(&fx.ctx, &key, "Investigating", 1800, "request-1", at(2000)).unwrap(),
        accepted
    );
    assert_eq!(fx.logical("attention_handled").len(), 2);
    let after = fx.ctx.store.get(&task.id).unwrap();
    let mut expected = before;
    expected.attention = after.attention.clone();
    assert_eq!(after, expected);
    assert!(fx.logical("attention_resolved").is_empty());
}

#[test]
fn resolving_one_key_keeps_the_other_keys_handling_and_levels() {
    // Given two independent questions and handling on the second.
    let fx = Fx::new();
    let task = fx.questions(2);
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    let keys: Vec<_> = fx
        .ctx
        .store
        .get(&task.id)
        .unwrap()
        .attention
        .records
        .keys()
        .cloned()
        .collect();
    attend(
        &fx.ctx,
        &keys[1].0,
        "Handling second",
        1800,
        "second",
        at(1),
    )
    .unwrap();
    let retained = fx.ctx.store.get(&task.id).unwrap().attention.records[&keys[1]].clone();
    // When only the first canonical question is disposed.
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut()
                .unwrap()
                .agent_host
                .as_mut()
                .unwrap()
                .questions
                .remove(0);
            Ok(())
        })
        .unwrap();
    reconcile(&fx.ctx, at(2), Some(true)).unwrap();
    // Then the first resolves with self-contained evidence and the second is untouched.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.attention.records.len(), 1);
    assert_eq!(after.attention.records[&keys[1]], retained);
    let fact = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "attention_resolved")
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
        assert!(fact.detail.get(field).is_some(), "{field}");
    }
}

#[test]
fn lifecycle_publication_crashes_replay_open_level_handling_and_resolution() {
    for point in [
        CrashPoint::AfterTransition,
        CrashPoint::AfterAppend,
        CrashPoint::BeforeClear,
    ] {
        for action in ["open", "level", "handling", "resolve"] {
            // Given a stop at each lifecycle publication boundary.
            let fx = Fx::new();
            let task = fx.stalled(None);
            let key = key(&fx.ctx, &task);
            if action != "open" {
                reconcile(&fx.ctx, at(0), Some(true)).unwrap();
            }
            if action == "resolve" {
                fx.ctx
                    .store
                    .update(&task.id, |task| {
                        task.status = Status::Done;
                        Ok(())
                    })
                    .unwrap();
            }
            CRASH.with(|crash| crash.set(Some(point)));
            let result = match action {
                "handling" => {
                    attend(&fx.ctx, &key, "Looking", 1800, "lost-output", at(1)).map(|_| ())
                }
                "level" => reconcile(&fx.ctx, at(1800), Some(true)),
                "open" | "resolve" => reconcile(&fx.ctx, at(1), Some(true)),
                _ => unreachable!(),
            };
            assert!(result.is_err(), "{point:?} {action}");
            if action == "resolve" {
                assert!(
                    !fx.ctx
                        .store
                        .get(&task.id)
                        .unwrap()
                        .attention
                        .records
                        .is_empty()
                );
            }
            // When a new board recovers, facts cannot disappear or cause another send.
            let reopened = fx.reopen();
            reconcile(&reopened, at(2), Some(true)).unwrap();
            reconcile(&reopened, at(2), Some(true)).unwrap();
            let expected = match action {
                "open" => "attention_opened",
                "level" => "attention_overdue",
                "handling" => "attention_handled",
                "resolve" => "attention_resolved",
                _ => unreachable!(),
            };
            assert!(!fx.logical(expected).is_empty(), "{point:?} {action}");
            assert!(
                reopened
                    .store
                    .get(&task.id)
                    .unwrap()
                    .attention
                    .outbox
                    .is_empty()
            );
            assert!(fx.calls() <= 1, "{point:?} {action}");
        }
    }
}

#[test]
fn project_digests_keep_complete_membership_beyond_the_display_cap() {
    // Given eighteen alpha tasks and two beta tasks opening in one pass.
    let fx = Fx::new();
    let mut alpha = Vec::new();
    for _ in 0..18 {
        alpha.push(fx.stalled(Some("alpha")).id);
    }
    for _ in 0..2 {
        fx.stalled(Some("beta"));
    }
    // When attention reconciles once.
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    // Then display truncation never truncates routing.
    let events = fx.ctx.store.events().unwrap();
    let due: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "attention_due")
        .collect();
    assert_eq!(due.len(), 2);
    let a = due
        .iter()
        .find(|event| event.detail["project"] == "alpha")
        .unwrap();
    assert_eq!(a.detail["count"], 18);
    assert_eq!(a.detail["entries"].as_array().unwrap().len(), 16);
    assert_eq!(a.detail["tasks"].as_array().unwrap().len(), 18);
    assert!(
        alpha
            .iter()
            .all(|id| a.detail["tasks"].as_array().unwrap().contains(&json!(id)))
    );
    assert!(a.task.is_none());
}

#[test]
fn overflow_never_evicts_and_frees_records_only_after_resolution_publication() {
    // Given thirty-three distinct canonical question conditions on one task.
    let fx = Fx::new();
    let task = fx.questions(33);
    // When the fixed ledger fills, then all conditions remain listed.
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    let filled = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(filled.attention.records.len(), 32);
    assert_eq!(filled.attention.overflow_count, Some(2));
    let rows = board(&fx.ctx, at(0))
        .unwrap()
        .tasks
        .remove(0)
        .assessment
        .conditions;
    assert_eq!(
        rows.iter()
            .filter(|row| row.kind == "question_open")
            .count(),
        33
    );
    let first = filled
        .attention
        .records
        .keys()
        .find(|key| key.0.contains("/question_open/"))
        .unwrap()
        .clone();
    let q = first
        .0
        .split("/question_open/")
        .nth(1)
        .unwrap()
        .split('@')
        .next()
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut()
                .unwrap()
                .agent_host
                .as_mut()
                .unwrap()
                .questions
                .retain(|question| question.key != q);
            Ok(())
        })
        .unwrap();
    reconcile(&fx.ctx, at(1), Some(true)).unwrap();
    reconcile(&fx.ctx, at(2), Some(true)).unwrap();
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.attention.records.len(), 32);
    assert_eq!(after.attention.overflow_count, Some(1));
    assert!(!after.attention.records.contains_key(&first));
}

#[test]
fn idle_reconcile_preserves_files_and_bounded_work_with_tenfold_done_history() {
    use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
    use corral_core::work::{Work, count};
    // Given an open condition already reconciled and warm task caches.
    let fx = Fx::new();
    let task = fx.questions(0);
    let inbox = Inbox::new(
        &fx.ctx.store,
        Recipient::Run {
            task: task.id.clone(),
            run: "R1".into(),
        },
    )
    .unwrap();
    inbox
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Current instruction",
            subject: None,
            binding: MessageBinding::default(),
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap();
    let grow_inbox = |range: std::ops::Range<u64>| {
        for index in range {
            std::fs::write(inbox.message_path(index), b"unreachable invalid history").unwrap();
        }
    };
    grow_inbox(100..200);
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    for _ in 0..10 {
        fx.ctx
            .store
            .create("History", None, Status::Done, "")
            .unwrap();
    }
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    let measure = || {
        let task = fx.ctx.store.get(&task.id).unwrap();
        let task_bytes = std::fs::read(&task.path).unwrap();
        let events = std::fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap();
        let before = [
            count(Work::TaskList),
            count(Work::TaskParse),
            count(Work::EventRead),
            count(Work::InboxRecordRead),
            count(Work::InboxMessageRead),
            count(Work::InboxEnvelopeRead),
            count(Work::InboxList),
        ];
        reconcile(&fx.ctx, at(0), Some(true)).unwrap();
        let after = [
            count(Work::TaskList),
            count(Work::TaskParse),
            count(Work::EventRead),
            count(Work::InboxRecordRead),
            count(Work::InboxMessageRead),
            count(Work::InboxEnvelopeRead),
            count(Work::InboxList),
        ];
        assert_eq!(std::fs::read(&task.path).unwrap(), task_bytes);
        assert_eq!(
            std::fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap(),
            events
        );
        std::array::from_fn::<_, 7, _>(|i| after[i] - before[i])
    };
    let baseline = measure();
    for _ in 0..90 {
        fx.ctx
            .store
            .create("History", None, Status::Done, "")
            .unwrap();
    }
    grow_inbox(200..1100);
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    // When unchanged passes run with 1x and 10x Done history, then added work is identical.
    assert_eq!(baseline, measure());
    assert_eq!(&baseline[..3], &[1, 0, 0]);
    assert!(baseline[3] > 0 && baseline[3] <= 6);
    assert_eq!(&baseline[4..], &[0, 0, 0]);
}

#[test]
fn two_owner_levels_share_one_batch_and_later_episodes_do_not_inherit_outcomes() {
    // Given two questions on one task.
    let fx = Fx::new();
    let task = fx.questions(2);
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    // When both reach the owner deadline, they share one physical batch.
    reconcile(&fx.ctx, at(1800), Some(true)).unwrap();
    let saved = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(fx.calls(), 1);
    let claims: BTreeSet<_> = saved
        .attention
        .records
        .values()
        .filter_map(|record| record.levels.owner_claim.as_ref())
        .collect();
    assert_eq!(claims.len(), 1);
    let claim = &saved.attention.notice_outbox[0];
    assert_eq!(claim.claim_ids.len(), 2);
    assert_eq!(claim.coverage.len(), 2);
    fx.ctx
        .store
        .update(&task.id, |task| {
            for q in &mut task
                .current_run_mut()
                .unwrap()
                .agent_host
                .as_mut()
                .unwrap()
                .questions
            {
                q.revision = 2;
                q.asked.at = at(2000);
            }
            Ok(())
        })
        .unwrap();
    reconcile(&fx.ctx, at(2000), Some(true)).unwrap();
    reconcile(&fx.ctx, at(3800), Some(true)).unwrap();
    // Then later source identity gives a new batch, not a join by producer/task.
    assert_eq!(fx.calls(), 2);
    let batches: BTreeSet<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "owner_notified")
        .map(|event| event.detail["batch_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(batches.len(), 2);
}

#[test]
fn ledger_serialization_has_enforced_count_note_and_byte_bounds() {
    // Given a full ledger with maximum-size keys, kind, request and handling note.
    let fx = Fx::new();
    let task = fx.questions(33);
    reconcile(&fx.ctx, at(0), Some(true)).unwrap();
    let mut ledger = fx.ctx.store.get(&task.id).unwrap().attention;
    ledger.records = ledger
        .records
        .into_iter()
        .enumerate()
        .map(|(index, (_, mut record))| {
            record.kind = "k".repeat(64);
            record.handling = Some(corral_core::attention::AttentionHandling {
                revision: u32::MAX,
                request: "r".repeat(128),
                note: "n".repeat(1024),
                at: at(0),
                next_check: at(86400),
                by: Responsible::Pm,
            });
            (
                ConditionKey(format!("{index:04}{}", "x".repeat(1020))),
                record,
            )
        })
        .collect();
    // When it is serialized, then measured retained-record bytes stay below the hard cap.
    let bytes = serde_json::to_vec(&ledger).unwrap().len();
    println!(
        "full maximum-size record fixture: {bytes} bytes; enforced ledger cap {MAX_ATTENTION_LEDGER_BYTES}"
    );
    assert!(bytes < MAX_ATTENTION_LEDGER_BYTES);
    fx.ctx
        .store
        .update_attention(&task.id, |current| {
            *current = ledger.clone();
            Ok(())
        })
        .unwrap();
    let result = fx.ctx.store.update_attention(&task.id, |current| {
        current
            .records
            .values_mut()
            .next()
            .unwrap()
            .handling
            .as_mut()
            .unwrap()
            .note
            .push('x');
        Ok(())
    });
    assert!(result.is_err());
    // Pending fact payloads share the same hard byte budget, including serialization overhead.
    ledger.outbox.push(corral_core::attention::AttentionFact {
        key: ledger.records.keys().next().unwrap().clone(),
        fact_id: "fixture/size".into(),
        kind: "attention_handled".into(),
        detail: json!({"padding":""}),
    });
    let overhead = serde_json::to_vec(&ledger).unwrap().len();
    ledger.outbox[0].detail["padding"] = json!("x".repeat(MAX_ATTENTION_LEDGER_BYTES - overhead));
    assert_eq!(
        serde_json::to_vec(&ledger).unwrap().len(),
        MAX_ATTENTION_LEDGER_BYTES
    );
    fx.ctx
        .store
        .update_attention(&task.id, |current| {
            *current = ledger.clone();
            Ok(())
        })
        .unwrap();
    let before = fx.ctx.store.get(&task.id).unwrap();
    assert!(
        fx.ctx
            .store
            .update_attention(&task.id, |current| {
                current.outbox[0].detail["padding"] = json!(format!(
                    "{}x",
                    current.outbox[0].detail["padding"].as_str().unwrap()
                ));
                Ok(())
            })
            .is_err()
    );
    assert_eq!(fx.ctx.store.get(&task.id).unwrap(), before);
    let legacy: AttentionLedger = serde_json::from_str("{}").unwrap();
    assert!(legacy.is_empty());
}
