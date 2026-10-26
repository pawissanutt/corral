use std::collections::BTreeSet;

use corral_core::attention::ConditionKey;

use super::fixture::{Fx, at};

fn sync(fx: &Fx, seconds: i64) {
    corral_ops::sync_at(&fx.reopen(), at(seconds)).unwrap();
}

fn overflow_key(fx: &Fx, id: &str) -> ConditionKey {
    fx.ctx
        .store
        .get(id)
        .unwrap()
        .attention
        .records
        .into_iter()
        .find(|(_, record)| record.kind == "ledger_overflow")
        .unwrap()
        .0
}

#[test]
fn resolved_overflow_never_advances_at_a_deadline_with_or_without_other_conditions() {
    for retain_recorded in [false, true] {
        // Given overflow whose excess questions disappear before the next deadline pass.
        let fx = Fx::new();
        let task = fx.questions(33);
        sync(&fx, 0);
        let saved = fx.ctx.store.get(&task.id).unwrap();
        let key = overflow_key(&fx, &task.id);
        fx.ctx
            .store
            .update(&task.id, |task| {
                let id = task.id.clone();
                task.current_run_mut()
                    .unwrap()
                    .agent_host
                    .as_mut()
                    .unwrap()
                    .questions
                    .retain(|question| {
                        retain_recorded
                            && saved.attention.records.contains_key(&ConditionKey(format!(
                                "{id}/question_open/{}@{}",
                                question.key, question.revision
                            )))
                    });
                Ok(())
            })
            .unwrap();
        // When fresh canonical conditions are reconciled at the owner deadline.
        sync(&fx, 1800);
        // Then overflow resolves, with no new overflow level, claim or physical send.
        let events = fx.ctx.store.events().unwrap();
        assert!(
            !events
                .iter()
                .any(|event| event.kind == "attention_overdue" && event.detail["key"] == key.0)
        );
        assert!(
            !events
                .iter()
                .filter(|event| event.kind == "notification_claimed")
                .any(|event| event.detail["coverage"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|coverage| coverage["condition_key"] == key.0))
        );
        assert_eq!(fx.calls(), usize::from(retain_recorded));
        let after = fx.ctx.store.get(&task.id).unwrap();
        assert!(!after.attention.records.contains_key(&key));
        assert_eq!(after.attention.overflow_count, None);
        assert_eq!(
            after.attention.records.len(),
            if retain_recorded { 31 } else { 0 }
        );
        let resolved = events
            .iter()
            .find(|event| event.kind == "attention_resolved" && event.detail["key"] == key.0)
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
    }
}

#[test]
fn overflow_identity_survives_count_changes_and_restart() {
    // Given a committed overflow episode.
    let fx = Fx::new();
    let task = fx.questions(33);
    sync(&fx, 0);
    let saved = fx.ctx.store.get(&task.id).unwrap();
    let key = overflow_key(&fx, &task.id);
    // When one unrecorded question disappears and a fresh process reconciles twice.
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
            let index = questions
                .iter()
                .position(|question| {
                    !saved.attention.records.contains_key(&ConditionKey(format!(
                        "{id}/question_open/{}@{}",
                        question.key, question.revision
                    )))
                })
                .unwrap();
            questions.remove(index);
            Ok(())
        })
        .unwrap();
    sync(&fx, 1);
    sync(&fx, 2);
    // Then count and current source age change without creating another episode.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.attention.overflow_count, Some(1));
    assert_eq!(overflow_key(&fx, &task.id), key);
    assert_eq!(after.attention.records[&key], saved.attention.records[&key]);
    let board = corral_ops::attention::board(&fx.reopen(), at(2)).unwrap();
    let condition = board
        .tasks
        .iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .iter()
        .find(|condition| condition.kind == "ledger_overflow")
        .unwrap();
    assert_eq!(condition.key.as_ref(), Some(&key));
    assert_eq!(condition.since, Some(at(0)));
    assert_eq!(condition.age_secs, Some(2));
}

#[test]
fn reopened_overflow_has_distinct_logical_openings() {
    // Given an overflow episode which has resolved and been pruned under a frozen clock.
    let fx = Fx::new();
    let task = fx.questions(33);
    let original = task
        .current_run()
        .unwrap()
        .agent_host
        .as_ref()
        .unwrap()
        .questions
        .clone();
    sync(&fx, 0);
    let first = overflow_key(&fx, &task.id);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut()
                .unwrap()
                .agent_host
                .as_mut()
                .unwrap()
                .questions
                .clear();
            Ok(())
        })
        .unwrap();
    sync(&fx, 0);
    assert!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .records
            .is_empty()
    );
    // When new canonical question revisions open another episode at the same timestamp.
    fx.ctx
        .store
        .update(&task.id, |task| {
            let host = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
            host.questions = original;
            for question in &mut host.questions {
                question.revision = 2;
                question.asked.at = at(0);
            }
            Ok(())
        })
        .unwrap();
    sync(&fx, 0);
    // Then exact keys and deduplicated lifecycle populations distinguish both episodes.
    assert_ne!(overflow_key(&fx, &task.id), first);
    let events = fx.ctx.store.events().unwrap();
    let openings: Vec<_> = events
        .iter()
        .filter(|event| {
            event.kind == "attention_opened" && event.detail["kind"] == "ledger_overflow"
        })
        .collect();
    assert_eq!(openings.len(), 2);
    assert_eq!(
        openings
            .iter()
            .map(|event| event.detail["fact_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
}

#[test]
fn separate_overflow_episodes_keep_distinct_physical_owner_batches() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "overflow::owner_episodes_subprocess",
            "--nocapture",
        ])
        .env("T430_OVERFLOW_EPISODES", "1")
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
}

#[test]
fn owner_episodes_subprocess() {
    if std::env::var_os("T430_OVERFLOW_EPISODES").is_none() {
        return;
    }
    // Given two separate episodes whose ordinary questions are explicitly handled.
    let fx = Fx::new();
    let task = fx.questions(33);
    let original = task
        .current_run()
        .unwrap()
        .agent_host
        .as_ref()
        .unwrap()
        .questions
        .clone();
    let mut keys = BTreeSet::new();
    for (revision, start) in [(1, 0), (2, 1803)] {
        if revision == 2 {
            fx.ctx
                .store
                .update(&task.id, |task| {
                    let host = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
                    host.questions = original.clone();
                    for question in &mut host.questions {
                        question.revision = revision;
                        question.asked.at = at(start);
                    }
                    Ok(())
                })
                .unwrap();
        }
        sync(&fx, start);
        let overflow = overflow_key(&fx, &task.id);
        keys.insert(overflow.0.clone());
        for key in fx
            .ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .records
            .into_keys()
        {
            corral_ops::attention::attend(
                &fx.ctx,
                &key.0,
                "Investigating",
                if key == overflow { 60 } else { 86400 },
                "same-request",
                at(start + 1),
            )
            .unwrap();
        }
        // When the unhandled overflow alone reaches its owner deadline.
        sync(&fx, start + 1800);
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.current_run_mut()
                    .unwrap()
                    .agent_host
                    .as_mut()
                    .unwrap()
                    .questions
                    .clear();
                Ok(())
            })
            .unwrap();
        sync(&fx, start + 1801);
        sync(&fx, start + 1802);
    }
    // Then both physical sends, claim coverage, handling and opening facts have episode identities.
    let events = fx.ctx.store.events().unwrap();
    let batches: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "owner_notified")
        .collect();
    assert_eq!(fx.calls(), 2);
    assert_eq!(batches.len(), 2);
    assert_ne!(batches[0].detail["batch_id"], batches[1].detail["batch_id"]);
    assert_ne!(
        batches[0].detail["claim_ids"],
        batches[1].detail["claim_ids"]
    );
    assert_eq!(keys.len(), 2);
    for kind in [
        "attention_opened",
        "attention_handled",
        "attention_resolved",
    ] {
        let facts: BTreeSet<_> = events
            .iter()
            .filter(|event| {
                event.kind == kind
                    && keys.contains(event.detail["key"].as_str().unwrap_or_default())
            })
            .map(|event| event.detail["fact_id"].as_str().unwrap())
            .collect();
        assert_eq!(facts.len(), 2, "{kind}");
    }
}
