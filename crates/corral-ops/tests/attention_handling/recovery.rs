use corral_core::attention::{AttentionFact, AttentionHandling, ConditionKey, Responsible};
use corral_core::{Status, Task};
use serde_json::json;

use super::fixture::{Fx, at};

fn recorded(fx: &Fx) -> (Task, ConditionKey) {
    let task = fx.stalled(None);
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    let key = fx
        .ctx
        .store
        .get(&task.id)
        .unwrap()
        .attention
        .records
        .into_keys()
        .next()
        .unwrap();
    (task, key)
}

fn handling(revision: u32) -> AttentionHandling {
    AttentionHandling {
        revision,
        request: format!("request-{revision}"),
        note: format!("note-{revision}"),
        at: at(i64::from(revision)),
        next_check: at(i64::from(revision) + 60),
        by: Responsible::Pm,
    }
}

fn pending(fx: &Fx, key: &ConditionKey, count: u32) {
    let id = key.0.split('/').next().unwrap();
    // Exact legal post-transition state: each handling committed before publication.
    fx.ctx
        .store
        .update_attention(id, |ledger| {
            for revision in 1..=count {
                let value = handling(revision);
                let record = ledger.records.get_mut(key).unwrap();
                record.handling = Some(value.clone());
                record.first_handled_at.get_or_insert(value.at);
                ledger.outbox.push(AttentionFact {
                    key: key.clone(),
                    fact_id: format!("{key}/attention_handled/{revision}"),
                    kind: "attention_handled".into(),
                    detail: json!({"key": key, "handling": value}),
                });
            }
            Ok(())
        })
        .unwrap();
}

fn public_attend(
    fx: &Fx,
    key: &ConditionKey,
    value: &AttentionHandling,
) -> Result<AttentionHandling, String> {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "recovery::attend_subprocess", "--nocapture"])
        .env("ATTENTION_RECOVERY_ROOT", fx.ctx.store.root())
        .env("ATTENTION_RECOVERY_KEY", &key.0)
        .env(
            "ATTENTION_RECOVERY_PAYLOAD",
            serde_json::to_string(value).unwrap(),
        )
        .env("CORRAL_PM", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let result = stdout
        .lines()
        .find_map(|line| line.strip_prefix("ATTEND_RESULT="))
        .unwrap();
    serde_json::from_str(result).unwrap()
}

#[test]
fn attend_subprocess() {
    let Ok(root) = std::env::var("ATTENTION_RECOVERY_ROOT") else {
        return;
    };
    let ctx =
        corral_ops::Ctx::from_store(corral_core::Store::open(std::path::Path::new(&root)).unwrap());
    let key = std::env::var("ATTENTION_RECOVERY_KEY").unwrap();
    let value: AttentionHandling =
        serde_json::from_str(&std::env::var("ATTENTION_RECOVERY_PAYLOAD").unwrap()).unwrap();
    let result = corral_ops::attention::attend(
        &ctx,
        &key,
        &value.note,
        u64::try_from(value.next_check.duration_since(value.at).as_secs()).unwrap(),
        &value.request,
        value.at,
    )
    .map_err(|error| error.to_string());
    println!("ATTEND_RESULT={}", serde_json::to_string(&result).unwrap());
}

#[test]
fn full_outbox_recovers_expired_handling_and_both_due_levels() {
    // Given four unpublished handling facts and an expired latest handling.
    let fx = Fx::new();
    let (task, key) = recorded(&fx);
    pending(&fx, &key, 4);
    // Unknown inventory keeps PM liveness honest and permits the due re-wake.
    std::fs::write(fx.tmp.path().join("inventory-down"), "").unwrap();
    // When real sync reaches both deadlines across a reopened context.
    corral_ops::sync_at(&fx.reopen(), at(2000)).unwrap();
    corral_ops::sync_at(&fx.reopen(), at(2001)).unwrap();
    // Then all facts drain, both levels advance and transport remains at most once.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert!(after.attention.outbox.is_empty());
    assert_eq!(
        after.attention.records[&key].levels.rewoke_at,
        Some(at(2000))
    );
    assert_eq!(
        after.attention.records[&key].levels.owner_claimed_at,
        Some(at(2000))
    );
    assert_eq!(fx.logical("attention_handled").len(), 4);
    assert_eq!(fx.logical("attention_overdue").len(), 2);
    assert_eq!(fx.calls(), 1);
}

#[test]
fn full_outbox_recovers_a_due_rewake_without_owner_escalation() {
    // Given a valid four-fact outbox before the PM deadline.
    let fx = Fx::new();
    let (task, key) = recorded(&fx);
    pending(&fx, &key, 4);
    std::fs::write(fx.tmp.path().join("inventory-down"), "").unwrap();
    // When just the re-wake becomes due.
    corral_ops::sync_at(&fx.reopen(), at(900)).unwrap();
    // Then reclaim precedes the level transition.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert!(after.attention.outbox.is_empty());
    assert_eq!(
        after.attention.records[&key].levels.rewoke_at,
        Some(at(900))
    );
    assert_eq!(after.attention.records[&key].levels.owner_claimed_at, None);
    assert_eq!(fx.logical("attention_overdue").len(), 1);
    assert_eq!(fx.calls(), 0);
}

#[test]
fn full_outbox_publishes_terminal_resolution_before_pruning() {
    // Given a full outbox on a newly terminal canonical task.
    let fx = Fx::new();
    let (task, key) = recorded(&fx);
    pending(&fx, &key, 4);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    // When terminal recovery runs.
    corral_ops::sync_at(&fx.reopen(), at(5)).unwrap();
    // Then all handling history and the complete resolution survive record removal.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert!(after.attention.records.is_empty());
    assert!(after.attention.outbox.is_empty());
    assert_eq!(fx.logical("attention_handled").len(), 4);
    let events = fx.ctx.store.events().unwrap();
    let resolutions: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "attention_resolved")
        .collect();
    assert_eq!(resolutions.len(), 1);
    assert_eq!(resolutions[0].detail["first_handled_at"], json!(at(1)));
    assert_eq!(
        resolutions[0].detail["record"]["handling"],
        json!(handling(4))
    );
}

#[test]
fn new_handling_reclaims_full_published_capacity_before_revision_five() {
    // Given four committed unpublished requests.
    let fx = Fx::new();
    let (task, key) = recorded(&fx);
    pending(&fx, &key, 4);
    // When a genuinely new request is accepted through the public API.
    let result = public_attend(&fx, &key, &handling(5)).unwrap();
    // Then it is revision five, without dropping any of the four previous facts.
    assert_eq!(result, handling(5));
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.attention.records[&key].handling, Some(result));
    assert!(after.attention.outbox.is_empty());
    assert_eq!(fx.logical("attention_handled").len(), 5);
}

#[test]
fn replay_and_payload_checks_cover_latest_pending_and_published_requests() {
    for source in ["latest", "older_pending", "older_published"] {
        // Given two accepted requests, either pending or published.
        let fx = Fx::new();
        let (task, key) = recorded(&fx);
        pending(&fx, &key, 2);
        if source == "older_published" {
            corral_ops::sync_at(&fx.ctx, at(3)).unwrap();
        }
        let prior = handling(if source == "latest" { 2 } else { 1 });
        let before = fx.ctx.store.get(&task.id).unwrap();
        let events = fx.ctx.store.events().unwrap();
        // When an accepted payload is replayed, or either payload field is changed.
        assert_eq!(public_attend(&fx, &key, &prior).unwrap(), prior, "{source}");
        for change in ["note", "next_check"] {
            let mut changed = prior.clone();
            match change {
                "note" => changed.note.push('!'),
                "next_check" => {
                    changed.next_check = changed
                        .next_check
                        .checked_add(jiff::SignedDuration::from_secs(60))
                        .unwrap()
                }
                _ => unreachable!(),
            }
            assert_eq!(
                public_attend(&fx, &key, &changed).unwrap_err(),
                "request_payload_mismatch",
                "{source} {change}",
            );
        }
        // Then no revision, task byte or event changes for any accepted-request lookup.
        assert_eq!(fx.ctx.store.get(&task.id).unwrap(), before, "{source}");
        assert_eq!(fx.ctx.store.events().unwrap(), events, "{source}");
    }
}

#[test]
fn attend_before_sync_announces_once_and_pauses_only_its_later_levels() {
    // Given a canonical condition that has never been reconciled.
    let fx = Fx::new();
    std::fs::write(fx.tmp.path().join("inventory-down"), "").unwrap();
    let task = fx.stalled(None);
    let key = corral_ops::attention::board(&fx.ctx, at(0))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find_map(|c| c.key)
        .unwrap();
    let mut value = handling(1);
    value.next_check = at(1801);
    // When handling creates the record before its first sync.
    public_attend(&fx, &key, &value).unwrap();
    for seconds in [2, 3, 1800] {
        corral_ops::sync_at(&fx.reopen(), at(seconds)).unwrap();
    }
    // Then the opening announces once even while later levels remain paused.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(
        after.attention.records[&key].levels.announced_at,
        Some(at(1))
    );
    assert_eq!(after.attention.records[&key].levels.rewoke_at, None);
    assert_eq!(fx.logical("attention_due").len(), 1);
    assert_eq!(fx.calls(), 0);
    corral_ops::sync_at(&fx.reopen(), at(1801)).unwrap();
    assert_eq!(fx.logical("attention_due").len(), 2);
    assert_eq!(fx.calls(), 1);
}

#[test]
fn overflow_opening_joins_one_digest_and_never_announces_again() {
    // Given thirty-three canonical conditions needing the reserved overflow slot.
    let fx = Fx::new();
    let task = fx.questions(33);
    // When opening and unchanged sync passes run across restart.
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    corral_ops::sync_at(&fx.reopen(), at(1)).unwrap();
    corral_ops::sync_at(&fx.reopen(), at(2)).unwrap();
    // Then overflow and the 31 retained normal rows share one complete opening digest.
    let after = fx.ctx.store.get(&task.id).unwrap();
    let key = after
        .attention
        .records
        .iter()
        .find(|(_, record)| record.kind == "ledger_overflow")
        .unwrap()
        .0
        .clone();
    assert_eq!(
        after.attention.records[&key].levels.announced_at,
        Some(at(0))
    );
    assert_eq!(after.attention.records.len(), 32);
    assert_eq!(after.attention.overflow_count, Some(2));
    let events = fx.ctx.store.events().unwrap();
    let due: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "attention_due")
        .collect();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].detail["count"], 32);
    let opened: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "attention_opened" && e.detail["key"] == key.0)
        .collect();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].detail["wake"], true);
    assert_eq!(opened[0].detail["class"], "pm_action");
    assert!(!opened[0].detail["reason"].as_str().unwrap().is_empty());
}
