//! T-560: owner-queued work survives the policy-revision format change, and a meaningful
//! policy change holds it once as a Blocked launch until an explicit owner dispatch.
use super::*;
use sha2::{Digest, Sha256};

/// The pre-T-560 policy revision, computed independently of the current implementation: the
/// same tuple, then including `limits`, hashed as `content_hash(serde_json::to_string(..))`.
fn base_format_revision(fx: &Fx) -> String {
    let config = Store::open(fx.temp.path()).unwrap().config;
    assert!(
        config.review.plan_checkpoint_every.is_none(),
        "policy_view is identity here"
    );
    let mut dispatch = config.dispatch.clone();
    dispatch.paused = false;
    let text = serde_json::to_string(&(
        fx.ctx.store.root(),
        &config.automation,
        &config.review,
        &dispatch,
        &config.projects,
        &config.limits,
        &config.resources,
    ))
    .unwrap();
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Rewrite every persisted copy of an owner approval the way the base binary wrote it.
fn seed_base_format(fx: &Fx, id: &str) -> corral_core::ActionAuthorization {
    let path = fx
        .ctx
        .store
        .state_dir()
        .join("action-authorizations")
        .join(format!("{id}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let revision = base_format_revision(fx);
    assert_ne!(
        record["authorization"]["policy_revision"],
        revision.as_str()
    );
    record["authorization"]["policy_revision"] = revision.into();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let seeded: corral_core::ActionAuthorization =
        serde_json::from_value(record["authorization"].clone()).unwrap();
    let task = &seeded.action.task;
    fx.ctx
        .store
        .update(task, |t| {
            if let Some(launch) = t.launch.as_mut() {
                launch.authorization = Some(seeded.clone());
            }
            Ok(())
        })
        .unwrap();
    seeded
}

/// Leave the owner's queued start reserved and refused before its final submission, as a
/// pause at the IPC barrier does: a retained launch (coding) or prepared claim (planning).
fn retain(fx: &Fx, approval: &corral_core::ActionAuthorization) {
    let barrier = Barrier::subscribe(fx);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
        let mut release = barrier.arrival();
        corral_ops::dispatch_pause(&fx.fresh(), true).unwrap();
        release.write_all(b"R").unwrap();
        worker.join().unwrap();
    });
    drop(barrier);
    assert!(fx.records("submissions.jsonl").is_empty());
    let task = fx.ctx.store.get(&approval.action.task).unwrap();
    assert_eq!(task.runs.len() + task.plan_runs.len(), 1);
}

fn events(fx: &Fx, task: &str, kind: &str) -> Vec<serde_json::Value> {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(task) && e.kind == kind)
        .map(|e| e.detail)
        .collect()
}

/// The attention board's one Blocked condition for `task` (kind, responsible, reason).
fn blocked(fx: &Fx, task: &str) -> (String, String, String) {
    let board = corral_ops::attention::board(&fx.fresh(), now()).unwrap();
    let board = serde_json::to_value(&board).unwrap();
    let entries: Vec<&serde_json::Value> = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry.to_string().contains(&format!("\"{task}\"")))
        .collect();
    assert_eq!(entries.len(), 1, "{board}");
    let condition = &entries[0]["blocked"];
    let field = |name: &str| condition[name].as_str().unwrap_or_default().to_string();
    (field("kind"), field("responsible"), field("reason"))
}

#[test]
fn base_format_owner_approval_carries_across_the_upgrade_without_renewal() {
    for planning in [false, true] {
        for retained in [false, true] {
            // Given owner consent persisted by the base binary, queued or already reserved.
            let fx = Fx::new();
            let approval = super::pause::enqueue(&fx, planning);
            if retained {
                retain(&fx, &approval);
            }
            let seeded = seed_base_format(&fx, &approval.id);
            // When the upgraded binary drains the unchanged configuration.
            if retained {
                corral_ops::dispatch_pause(&fx.fresh(), false).unwrap();
            }
            for _ in 0..3 {
                corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            }
            // Then that exact approval submits once: no refusal, renewal or Blocked hold.
            let case = format!("planning={planning} retained={retained}");
            assert_eq!(fx.records("submissions.jsonl").len(), 1, "{case}");
            let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
            assert_eq!(record.authorization, seeded, "{case}");
            assert_eq!(
                record.disposition,
                SubmissionDisposition::Submitted,
                "{case}"
            );
            assert!(record.authorization_history.is_empty(), "{case}");
            assert!(events(&fx, &approval.action.task, "owner_consent_stale").is_empty());
        }
    }
}

#[test]
fn meaningful_change_holds_owner_queue_once_until_an_explicit_owner_dispatch() {
    for retained in [false, true] {
        // Given an owner-queued coding start, unreserved or retained before submission.
        let fx = Fx::new();
        let approval = super::pause::enqueue(&fx, false);
        let id = approval.action.task.clone();
        if retained {
            retain(&fx, &approval);
            corral_ops::dispatch_pause(&fx.fresh(), false).unwrap();
        }
        let released = events(&fx, &id, "dispatch_released").len();
        // When a consent-relevant policy input changes and the queue drains repeatedly.
        Store::open(fx.temp.path())
            .unwrap()
            .edit_config(|doc| doc.set_str("review.max_rounds", "7"))
            .unwrap();
        for _ in 0..4 {
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        }
        // Then nothing submits and the task holds one stable Blocked launch error, once.
        let case = format!("retained={retained}");
        assert!(fx.records("submissions.jsonl").is_empty(), "{case}");
        assert_eq!(events(&fx, &id, "owner_consent_stale").len(), 1, "{case}");
        assert_eq!(
            events(&fx, &id, "dispatch_released").len(),
            released,
            "{case}"
        );
        let held = fx.ctx.store.get(&id).unwrap();
        let launch = held.launch.as_ref().unwrap();
        assert!(!launch.reserved, "{case}");
        let error = launch.error.as_deref().unwrap();
        assert!(error.contains("queued owner consent is stale"), "{error}");
        assert!(error.contains(&format!("corral dispatch {id}")), "{error}");
        let (kind, _, reason) = blocked(&fx, &id);
        assert_eq!(
            (kind.as_str(), reason.as_str()),
            ("launch_failed", error),
            "{case}"
        );
        let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
        assert_eq!(record.authorization, approval, "{case}");
        assert!(record.authorization_history.is_empty(), "{case}");
        // When the owner explicitly dispatches the held card.
        corral_ops::dispatch(&fx.fresh(), &id, &Default::default()).unwrap();
        // Then fresh owner consent supersedes the stale one in place and the run submits once.
        assert_eq!(fx.records("submissions.jsonl").len(), 1, "{case}");
        let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
        assert_eq!(
            record.disposition,
            SubmissionDisposition::Submitted,
            "{case}"
        );
        assert_eq!(record.authorization_history.len(), 1, "{case}");
        assert_eq!(record.authorization_history[0].authorization, approval);
        assert!(matches!(
            record.authorization.origin,
            OperatingOrigin::ExplicitOwner { .. }
        ));
        assert_eq!(fx.ctx.store.get(&id).unwrap().runs.len(), 1, "{case}");
    }
}

#[test]
fn meaningful_change_leaves_owner_planning_one_blocked_hold_until_an_explicit_owner_plan() {
    for retained in [false, true] {
        // Given an owner-queued planning start, unreserved or retained before submission.
        let fx = Fx::new();
        let approval = super::pause::enqueue(&fx, true);
        let id = approval.action.task.clone();
        if retained {
            retain(&fx, &approval);
            corral_ops::dispatch_pause(&fx.fresh(), false).unwrap();
        }
        let failed = events(&fx, &id, "dispatch_failed").len();
        // When a consent-relevant policy input changes and the board syncs repeatedly.
        Store::open(fx.temp.path())
            .unwrap()
            .edit_config(|doc| doc.set_str("review.max_rounds", "7"))
            .unwrap();
        for _ in 0..4 {
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        }
        // Then nothing submits, at most one refusal is logged, and one owner hold is Blocked.
        let case = format!("retained={retained}");
        assert!(fx.records("submissions.jsonl").is_empty(), "{case}");
        assert!(
            events(&fx, &id, "dispatch_failed").len() <= failed + 1,
            "{case}"
        );
        let (kind, responsible, _) = blocked(&fx, &id);
        assert_eq!(responsible, "owner", "{case} {kind}");
        let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
        assert_eq!(record.authorization, approval, "{case}");
        assert_ne!(
            record.disposition,
            SubmissionDisposition::Submitted,
            "{case}"
        );
        // When the owner explicitly requests the plan, a planner launches under fresh consent.
        let opts = corral_ops::DispatchOpts {
            plan: true,
            ..Default::default()
        };
        corral_ops::dispatch(&fx.fresh(), &id, &opts).unwrap();
        assert_eq!(fx.records("submissions.jsonl").len(), 1, "{case}");
    }
}
