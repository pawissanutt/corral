//! Durable infrastructure waits against real Plan targets and the plugin-free fake engine.
use super::plan_targets::{dispatch, events, plan, reviewed};
use super::*;
use corral_core::InfraWaitClass;
use jiff::SignedDuration;

fn at(seconds: i64) -> Timestamp {
    now() + SignedDuration::from_secs(seconds)
}

fn plan_candidate(approved: bool) -> Fx {
    let (mut fx, plan, [a, _, _]) = plan();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    fx.grant(std::slice::from_ref(&plan.id));
    dispatch(&mut fx, &a);
    if approved {
        reviewed(&mut fx, &a, "a.txt", "A\n");
    } else {
        fx.change("a.txt", "A\n");
        fx.done();
        corral_ops::sync_at(&fx.ctx, now()).unwrap();
        assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
        assert!(fx.run().review.unwrap().verdict.is_none());
        // Reviewer launch uses the real clock. Align its canonical start and first
        // inventory sample with this fixture's explicit sync clock before measuring.
        fx.ctx
            .store
            .update(&fx.id, |task| {
                let run = task.current_run_mut().unwrap();
                run.review.as_mut().unwrap().at = now();
                run.idle_since = Some(now());
                let child = run.review_runs.last_mut().unwrap();
                child.started = now();
                child.review.as_mut().unwrap().at = now();
                child.idle_since = Some(now());
                child.agent_state = Some("idle".into());
                Ok(())
            })
            .unwrap();
    }
    fx
}

fn target_lock(fx: &Fx) -> PathBuf {
    let checkout = fx.run().plan_target.unwrap().checkout;
    let admin = git(&checkout, &["rev-parse", "--absolute-git-dir"]);
    let lock = Path::new(&admin).join("index.lock");
    std::fs::write(&lock, b"external git writer\n").unwrap();
    lock
}

fn file_state(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    (
        std::fs::read(path).unwrap(),
        std::fs::metadata(path).unwrap().modified().unwrap(),
    )
}

fn no_infra_escalation(fx: &Fx) {
    for kind in ["infra_hold", "owner_notified"] {
        assert!(events(fx, &fx.id, kind).is_empty(), "{kind}");
    }
    let transitions = events(fx, &fx.id, "autopilot");
    assert!(transitions.iter().all(|event| event["to"] != "escalated"));
    assert!(fx.herdr_log("notify.log").is_empty());
}

#[test]
fn live_plan_reviewer_survives_three_lock_passes_and_reuses_gate_after_clear() {
    // Given a live reviewer and an external target lock, neither receipt is stale.
    let fx = plan_candidate(false);
    let before = fx.task();
    let run = fx.run();
    let lock = target_lock(&fx);
    let lock_state = file_state(&lock);
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
    let waiting = fx.task();
    let wait = waiting.infra_wait.as_ref().unwrap();
    assert_eq!(wait.class, InfraWaitClass::Contention);
    assert_eq!(wait.marker, "index.lock");
    assert_eq!(wait.path, lock);
    assert_eq!(wait.first_seen, at(1));
    assert!(wait.first_mtime.is_some());
    assert!(wait.held_at.is_none());
    assert_eq!(waiting.current_run(), before.current_run());
    assert_eq!(waiting.rounds, before.rounds);
    assert_eq!(waiting.status, before.status);
    let task_state = file_state(&waiting.path);
    let event_count = fx.ctx.store.events().unwrap().len();

    // When three more deterministic passes observe the same lock.
    for seconds in [2, 3, 4] {
        corral_ops::sync_at(&fx.ctx, at(seconds)).unwrap();

        // Then no run, file, or event is rewritten and no reviewer is cleaned up.
        assert_eq!(fx.task(), waiting);
        assert_eq!(fx.run(), run);
        assert_eq!(file_state(&waiting.path), task_state);
        assert_eq!(file_state(&lock), lock_state);
        assert_eq!(fx.ctx.store.events().unwrap().len(), event_count);
        no_infra_escalation(&fx);
    }

    // When the writer releases the marker, the same reviewer can approve and land.
    std::fs::remove_file(&lock).unwrap();
    corral_ops::sync_at(&fx.ctx, at(5)).unwrap();
    assert!(fx.task().infra_wait.is_none());
    assert!(events(&fx, &fx.id, "infra_hold_cleared").is_empty());
    fx.verdict(Verdict::Approve, "Reviewed candidate.");
    corral_ops::sync_at(&fx.ctx, at(6)).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(fx.run().gate, run.gate);
    assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1);
    assert_eq!(events(&fx, &fx.id, "review_started").len(), 1);
    assert_eq!(events(&fx, &fx.id, "approved").len(), 1);
    assert!(events(&fx, &fx.id, "infra_hold_cleared").is_empty());
    no_infra_escalation(&fx);
}

#[test]
fn persistent_lock_holds_once_at_bound_across_ten_passes_and_reopen() {
    // Given the default 300-second contention bound and a live Plan reviewer.
    let mut fx = plan_candidate(false);
    let before = fx.task();
    let lock = target_lock(&fx);
    let lock_state = file_state(&lock);
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    let first = fx.task().infra_wait.unwrap();
    corral_ops::sync_at(&fx.ctx, at(299)).unwrap();
    assert!(fx.task().infra_wait.unwrap().held_at.is_none());
    no_infra_escalation(&fx);

    // When the bound is reached and repeated passes include a new Ctx on the same store.
    corral_ops::sync_at(&fx.ctx, at(300)).unwrap();
    let held = fx.task();
    let wait = held.infra_wait.as_ref().unwrap();
    assert_eq!(wait.first_seen, first.first_seen);
    assert_eq!(wait.first_mtime, first.first_mtime);
    assert_eq!(wait.held_at, Some(at(300)));
    let state = file_state(&held.path);
    for seconds in 301..311 {
        if seconds == 306 {
            fx.ctx = Ctx {
                store: Store::open(fx.ctx.store.root()).unwrap(),
                herdr: Herdr::with_bin(fx.bin_dir.join("herdr"), None),
                resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            };
        }
        corral_ops::sync_at(&fx.ctx, at(seconds)).unwrap();
        assert_eq!(fx.task(), held);
        assert_eq!(file_state(&held.path), state);
    }

    // Then Review and its receipts survive, with one durable claim and one notification.
    assert_eq!(held.status, Status::Review);
    assert_eq!(held.current_run(), before.current_run());
    assert_eq!(held.rounds, before.rounds);
    assert_eq!(file_state(&lock), lock_state);
    assert_eq!(events(&fx, &fx.id, "infra_hold").len(), 1);
    let notified = events(&fx, &fx.id, "owner_notified");
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0]["producer"], "infra_hold");
    assert_eq!(held.attention.notice_outbox.len(), 1);
    assert_eq!(held.attention.notice_outbox[0].producer, "infra_hold");
    assert_eq!(fx.herdr_log("notify.log").lines().count(), 1);
    let transitions = events(&fx, &fx.id, "autopilot");
    assert!(transitions.iter().all(|event| event["to"] != "escalated"));
    assert!(events(&fx, &fx.id, "approved").is_empty());

    // When a held episode clears, one clear event resumes the same review and gate.
    std::fs::remove_file(&lock).unwrap();
    corral_ops::sync_at(&fx.ctx, at(311)).unwrap();
    assert!(fx.task().infra_wait.is_none());
    assert_eq!(events(&fx, &fx.id, "infra_hold_cleared").len(), 1);
    fx.verdict(
        Verdict::Approve,
        "Reviewed candidate after the writer released its lock.",
    );
    corral_ops::sync_at(&fx.ctx, at(312)).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(fx.run().gate, before.current_run().unwrap().gate);
    assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1);
    assert_eq!(events(&fx, &fx.id, "review_started").len(), 1);
    assert_eq!(events(&fx, &fx.id, "infra_hold_cleared").len(), 1);
    assert_eq!(events(&fx, &fx.id, "infra_hold").len(), 1);
    assert_eq!(events(&fx, &fx.id, "owner_notified").len(), 1);
    assert_eq!(fx.herdr_log("notify.log").lines().count(), 1);
}

#[test]
fn bound_changes_under_recorded_wait_returns_to_worker_without_infra_escalation() {
    // Given a live review whose target has a recorded contention wait.
    let fx = plan_candidate(false);
    let lock = target_lock(&fx);
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
    let before = fx.task();
    let review = fx.run().review.unwrap();
    let attempt = corral_ops::review_callback_token(&review).unwrap();
    assert!(before.infra_wait.is_some());

    // When the bound reviewer reports a defect while the lock still exists.
    corral_ops::verdict_bound(
        &fx.ctx,
        &fx.id,
        Verdict::Changes,
        "Correct the widget.",
        Some(&attempt),
    )
    .unwrap();
    corral_ops::sync_at(&fx.ctx, at(301)).unwrap();

    // Then the defect takes precedence over the infrastructure hold bound.
    let changed = fx.task();
    assert_eq!(
        changed.status,
        Status::Running,
        "stage={:?}, reason={}, errors={:?}",
        fx.stage(),
        fx.reason(),
        events(&fx, &fx.id, "autopilot_error")
    );
    assert_eq!(changed.rounds, before.rounds + 1);
    assert!(changed.infra_wait.is_none());
    assert_eq!(events(&fx, &fx.id, "changes_requested").len(), 1);
    assert!(lock.exists());
    no_infra_escalation(&fx);
}

#[test]
fn approval_records_during_wait_but_cannot_land_before_clear() {
    // Given a current reviewer on a target with a recorded marker wait.
    let fx = plan_candidate(false);
    let lock = target_lock(&fx);
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
    let before = fx.run();

    // When the reviewer approves while the marker persists.
    fx.verdict(Verdict::Approve, "Candidate reviewed.");
    corral_ops::sync_at(&fx.ctx, at(2)).unwrap();

    // Then the receipt is retained, but neither automatic nor explicit landing passes.
    assert_eq!(fx.run().review.unwrap().verdict, Some(Verdict::Approve));
    assert_eq!(fx.task().status, Status::Review);
    assert!(events(&fx, &fx.id, "approved").is_empty());
    let error = corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, false)
        .unwrap_err();
    assert!(error.is::<corral_git::GitLockContention>(), "{error:#}");
    assert_eq!(fx.run().gate, before.gate);
    std::fs::remove_file(lock).unwrap();
    corral_ops::sync_at(&fx.ctx, at(3)).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1);
}

#[test]
fn recorded_wait_does_not_make_a_moved_candidate_verdict_fresh() {
    // Given a pending reviewer, a recorded target wait, and a new candidate commit.
    let fx = plan_candidate(false);
    let lock = target_lock(&fx);
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
    let before = fx.run();
    let attempt = corral_ops::review_callback_token(before.review.as_ref().unwrap()).unwrap();
    fx.change("a.txt", "different candidate\n");
    corral_git::commit_all(&before.worktree, "candidate changed").unwrap();

    // When the old callback tries to request changes.
    let result = corral_ops::verdict_bound(
        &fx.ctx,
        &fx.id,
        Verdict::Changes,
        "Old candidate.",
        Some(&attempt),
    );

    // Then no verdict or repair is admitted on the new head.
    assert!(result.is_err());
    assert!(fx.run().review.unwrap().verdict.is_none());
    assert_eq!(fx.run().gate, before.gate);
    assert!(events(&fx, &fx.id, "changes_requested").is_empty());
    assert!(lock.exists());
}

#[test]
fn abandoned_operation_holds_without_aborting_or_losing_the_reviewer() {
    // Given an abandoned operation marker in the owned linked checkout.
    let fx = plan_candidate(false);
    let run = fx.run();
    let admin = git(
        &run.plan_target.as_ref().unwrap().checkout,
        &["rev-parse", "--absolute-git-dir"],
    );
    let marker = Path::new(&admin).join("MERGE_HEAD");
    std::fs::write(&marker, run.base_sha.as_bytes()).unwrap();
    let bytes = file_state(&marker);

    // When its infrastructure bound passes.
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
    corral_ops::sync_at(&fx.ctx, at(301)).unwrap();
    corral_ops::sync_at(&fx.ctx, at(302)).unwrap();

    // Then it remains an operation wait, not a destroyed review round.
    let task = fx.task();
    let wait = task.infra_wait.unwrap();
    assert_eq!(wait.class, InfraWaitClass::Operation);
    assert_eq!(wait.path, marker);
    assert_eq!(wait.first_seen, at(1));
    assert_eq!(wait.held_at, Some(at(301)));
    assert_eq!(fx.run(), run);
    assert_eq!(file_state(&marker), bytes);
    assert_eq!(events(&fx, &fx.id, "infra_hold").len(), 1);
    assert_eq!(events(&fx, &fx.id, "owner_notified").len(), 1);
    assert_eq!(fx.herdr_log("notify.log").lines().count(), 1);
}

#[test]
fn repair_key_failure_escalates_without_replacing_gate_or_evidence() {
    // Given a pending ordinary reviewer and a base that no longer resolves.
    let fx = Fx::new(|policy| {
        policy.gate = gate(&["true"]);
        policy.reviewer = Some("rev".into());
        policy.accept = Some(Accept::Auto);
    });
    fx.change("a.txt", "A\n");
    fx.done();
    fx.sync();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().base = "missing-base".into();
            Ok(())
        })
        .unwrap();
    let before = fx.run();

    // When preflight cannot build its RepairKey.
    fx.sync();

    // Then the original unclassified escalation remains, with its receipts intact.
    assert_eq!(fx.task().status, Status::NeedsMe);
    let reason = fx.reason();
    assert!(reason.starts_with("integration preflight infrastructure error:"));
    assert_eq!(fx.run().gate, before.gate);
    assert_eq!(fx.run().completion, before.completion);
    assert_eq!(
        events(&fx, &fx.id, "owner_notified")[0]["producer"],
        "autopilot_escalate"
    );
    assert!(fx.task().infra_wait.is_none());
}

#[test]
fn changes_under_marker_still_requires_its_published_plan_prerequisite() {
    for prerequisite_done in [true, false] {
        // Given a dependent candidate built from a genuinely landed prerequisite.
        let (mut fx, plan, [a, _, c]) = plan();
        fx.ctx.store.config.review.accept = Some(Accept::Auto);
        fx.ctx.store.save_config().unwrap();
        fx.grant(std::slice::from_ref(&plan.id));
        dispatch(&mut fx, &a);
        reviewed(&mut fx, &a, "a.txt", "A\n");
        corral_ops::approve(&fx.ctx, &a, ApproveStrategy::FastForward, true, false).unwrap();
        dispatch(&mut fx, &c);
        fx.change("c.txt", "C\n");
        fx.done();
        corral_ops::sync_at(&fx.ctx, now()).unwrap();
        assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
        let lock = target_lock(&fx);
        let lock_state = file_state(&lock);
        corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
        fx.verdict(Verdict::Changes, "Correct the dependent candidate.");
        if !prerequisite_done {
            fx.ctx
                .store
                .update(&a, |task| {
                    task.status = Status::NeedsMe;
                    Ok(())
                })
                .unwrap();
        }

        // When repair admission rechecks the prerequisite through the target marker.
        corral_ops::sync_at(&fx.ctx, at(2)).unwrap();

        // Then a published qualifying receipt works, but a reopened prerequisite blocks.
        assert_eq!(
            fx.task().status,
            if prerequisite_done {
                Status::Running
            } else {
                Status::Review
            }
        );
        assert_eq!(
            events(&fx, &fx.id, "changes_requested").len(),
            usize::from(prerequisite_done)
        );
        assert_eq!(fx.task().rounds, u32::from(prerequisite_done));
        assert_eq!(fx.task().infra_wait.is_none(), prerequisite_done);
        assert_eq!(file_state(&lock), lock_state);
        no_infra_escalation(&fx);
    }
}

#[test]
fn reviewed_fresh_receipts_obey_recorded_wait_before_acceptance_fast_path() {
    // Given approved receipts already at Reviewed, plus a persisted preflight wait.
    let fx = plan_candidate(true);
    let head = fx.run().review.unwrap().sha;
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().autopilot = Some(corral_core::AutopilotState {
                stage: AutoStage::Reviewed,
                reason: None,
                sha: Some(head),
                at: at(0),
            });
            Ok(())
        })
        .unwrap();
    let lock = target_lock(&fx);
    let mtime = Timestamp::try_from(std::fs::metadata(&lock).unwrap().modified().unwrap()).unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.infra_wait = Some(corral_core::InfraWait {
                version: 1,
                class: InfraWaitClass::Contention,
                marker: "index.lock".into(),
                path: lock.clone(),
                first_seen: at(1),
                first_mtime: Some(mtime),
                held_at: None,
            });
            Ok(())
        })
        .unwrap();
    let waiting = fx.task();
    let bytes = file_state(&waiting.path);

    // When fresh receipts would normally bypass the integration preflight.
    corral_ops::sync_at(&fx.ctx, at(2)).unwrap();

    // Then the persisted wait wins without stage change, acceptance, or replacement gate.
    assert_eq!(fx.task(), waiting);
    assert_eq!(file_state(&waiting.path), bytes);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
    assert!(events(&fx, &fx.id, "approved").is_empty());
    assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1);
    no_infra_escalation(&fx);

    // When the marker clears, acceptance progresses in that same sync.
    std::fs::remove_file(&lock).unwrap();
    corral_ops::sync_at(&fx.ctx, at(3)).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert!(fx.task().infra_wait.is_none());
    assert!(events(&fx, &fx.id, "infra_hold_cleared").is_empty());
    assert_eq!(events(&fx, &fx.id, "approved").len(), 1);
    assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1);
}

fn first_marker_wait(stage: AutoStage, marker: &str) {
    // Given real passing receipts that reached Reviewed without acceptance authority.
    let (mut fx, plan, [a, _, _]) = plan();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [plan.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", now())
        .unwrap();
    dispatch(&mut fx, &a);
    reviewed(&mut fx, &a, "a.txt", "A\n");
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut()
                .unwrap()
                .autopilot
                .as_mut()
                .unwrap()
                .stage = stage;
            Ok(())
        })
        .unwrap();
    let before = fx.task();
    assert!(before.infra_wait.is_none());
    let checkout = fx.run().plan_target.unwrap().checkout;
    let admin = git(&checkout, &["rev-parse", "--absolute-git-dir"]);
    let path = Path::new(&admin).join(marker);
    std::fs::write(&path, b"external writer\n").unwrap();
    let marker_state = file_state(&path);

    // When a marker first appears in a landing-capable stage, with no seeded wait.
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();

    // Then sync records a task-local wait before freshness reads can abort the pass.
    let waiting = fx.task();
    let wait = waiting.infra_wait.as_ref().unwrap();
    assert_eq!(wait.marker, marker);
    assert_eq!(wait.path, path);
    assert_eq!(wait.first_seen, at(1));
    assert_eq!(
        wait.class,
        match marker {
            "index.lock" => InfraWaitClass::Contention,
            "MERGE_HEAD" => InfraWaitClass::Operation,
            _ => unreachable!(),
        }
    );
    assert!(wait.held_at.is_none());
    assert_eq!(waiting.status, Status::Review);
    assert_eq!(waiting.current_run(), before.current_run());
    assert_eq!(waiting.rounds, before.rounds);
    let waiting_file = file_state(&waiting.path);
    for seconds in [2, 300] {
        corral_ops::sync_at(&fx.ctx, at(seconds)).unwrap();
        assert_eq!(fx.task(), waiting);
        assert_eq!(file_state(&waiting.path), waiting_file);
    }
    no_infra_escalation(&fx);

    // When the bound is reached, only one hold/notice is committed across later passes.
    corral_ops::sync_at(&fx.ctx, at(301)).unwrap();
    let held = fx.task();
    assert_eq!(held.infra_wait.as_ref().unwrap().held_at, Some(at(301)));
    assert_eq!(held.current_run(), before.current_run());
    corral_ops::sync_at(&fx.ctx, at(302)).unwrap();
    assert_eq!(fx.task(), held);
    assert_eq!(file_state(&path), marker_state);
    assert_eq!(events(&fx, &fx.id, "infra_hold").len(), 1);
    let notified = events(&fx, &fx.id, "owner_notified");
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0]["producer"], "infra_hold");
    assert_eq!(fx.herdr_log("notify.log").lines().count(), 1);
    assert!(events(&fx, &fx.id, "approved").is_empty());
    assert!(events(&fx, &fx.id, "autopilot_error").is_empty());

    // When acceptance is delegated, the marker still fences it until it clears.
    fx.grant(std::slice::from_ref(&plan.id));
    corral_ops::sync_at(&fx.ctx, at(303)).unwrap();
    assert_eq!(fx.task(), held);
    std::fs::remove_file(&path).unwrap();
    corral_ops::sync_at(&fx.ctx, at(304)).unwrap();

    // Then the clearing pass lands with the original verification and no defect round.
    assert_eq!(fx.task().status, Status::Done);
    assert!(fx.task().infra_wait.is_none());
    assert_eq!(fx.task().rounds, before.rounds);
    assert_eq!(fx.run().gate, before.current_run().unwrap().gate);
    assert_eq!(fx.run().review, before.current_run().unwrap().review);
    assert_eq!(
        fx.run().completion,
        before.current_run().unwrap().completion
    );
    assert_eq!(events(&fx, &fx.id, "infra_hold_cleared").len(), 1);
    assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1);
    assert_eq!(events(&fx, &fx.id, "review_started").len(), 1);
    assert_eq!(events(&fx, &fx.id, "approved").len(), 1);
    assert_eq!(events(&fx, &fx.id, "infra_hold").len(), 1);
    assert_eq!(events(&fx, &fx.id, "owner_notified").len(), 1);
    assert_eq!(fx.herdr_log("notify.log").lines().count(), 1);
}

#[test]
fn first_marker_in_reviewed_records_contention_wait() {
    first_marker_wait(AutoStage::Reviewed, "index.lock");
}

#[test]
fn first_marker_in_reviewed_records_operation_wait() {
    first_marker_wait(AutoStage::Reviewed, "MERGE_HEAD");
}

#[test]
fn first_marker_in_landing_records_contention_wait() {
    first_marker_wait(AutoStage::Landing, "index.lock");
}

#[test]
fn first_marker_in_landing_records_operation_wait() {
    first_marker_wait(AutoStage::Landing, "MERGE_HEAD");
}

#[test]
fn first_marker_in_blocked_records_contention_wait() {
    first_marker_wait(AutoStage::Blocked, "index.lock");
}

#[test]
fn first_marker_in_blocked_records_operation_wait() {
    first_marker_wait(AutoStage::Blocked, "MERGE_HEAD");
}

fn audited_plan_reviewer() -> Fx {
    let (mut fx, plan, [a, _, _]) = plan();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    let task = fx.ctx.store.get(&a).unwrap();
    let criteria = serde_json::from_value(serde_json::json!({
        "version": 1,
        "spec_digest": corral_ops::completion_spec_digest(&task).unwrap(),
        "reviewed_by": "owner",
        "audited": true,
        "require_gate": true,
        "criteria": [{
            "id": "C1", "required": true, "live": false,
            "scenario": "the widget spins", "expected": "pass"
        }],
        "owner_decisions": []
    }))
    .unwrap();
    corral_ops::register_criteria(&fx.ctx, &a, &criteria).unwrap();
    fx.grant(std::slice::from_ref(&plan.id));
    dispatch(&mut fx, &a);
    let out = fx._tmp.path().join("proof");
    std::fs::create_dir_all(&out).unwrap();
    super::tiers::worker_round(&fx, &out, 1);
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(fx.run().completion.evidence.len(), 1);
    fx
}

#[test]
fn dirty_or_changed_plan_identity_escalates_once_without_erasing_receipts() {
    for case in ["dirty target", "detached target", "changed binding"] {
        // Given a live reviewed candidate with nonempty, gate-bound evidence.
        let fx = audited_plan_reviewer();
        let checkout = fx.run().plan_target.unwrap().checkout;
        match case {
            "dirty target" => {
                std::fs::write(checkout.join("README.md"), "owner edit\n").unwrap();
            }
            "detached target" => {
                git(&checkout, &["checkout", "--detach", "-q"]);
            }
            "changed binding" => {
                fx.ctx
                    .store
                    .update(&fx.id, |task| {
                        task.current_run_mut()
                            .unwrap()
                            .plan_target
                            .as_mut()
                            .unwrap()
                            .scope_revision += 1;
                        Ok(())
                    })
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = fx.run();
        let reviewer = before.review_runs.last().unwrap();
        assert!(reviewer.ended.is_none());
        let head = git(&before.worktree, &["rev-parse", "HEAD"]);
        let target_head = git(&checkout, &["rev-parse", "HEAD"]);

        // When repeated syncs encounter a genuine target fault rather than contention.
        for seconds in [1, 2, 3] {
            corral_ops::sync_at(&fx.ctx, at(seconds)).unwrap();
        }

        // Then one owner escalation preserves candidate, gate, review, and evidence.
        let after = fx.run();
        assert_eq!(fx.task().status, Status::NeedsMe, "{case}");
        assert_eq!(fx.stage(), Some(AutoStage::Escalated), "{case}");
        assert_eq!(after.gate, before.gate, "{case}");
        let mut released_review = before.review.clone().unwrap();
        released_review.process_lost = true;
        assert_eq!(after.review, Some(released_review), "{case}");
        assert_eq!(after.completion, before.completion, "{case}");
        assert_eq!(git(&before.worktree, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), target_head);
        assert_eq!(events(&fx, &fx.id, "gate_started").len(), 1, "{case}");
        let escalated: Vec<_> = events(&fx, &fx.id, "autopilot")
            .into_iter()
            .filter(|event| event["to"] == "escalated")
            .collect();
        assert_eq!(escalated.len(), 1, "{case}");
        let released: Vec<_> = after
            .review_runs
            .iter()
            .filter(|run| {
                before
                    .review_runs
                    .iter()
                    .any(|old| old.id == run.id && old.ended.is_none() && run.ended.is_some())
            })
            .map(|run| run.id.clone())
            .collect();
        assert_eq!(
            released.as_slice(),
            std::slice::from_ref(&reviewer.id),
            "{case}"
        );
        assert_eq!(
            escalated[0]["released_review_runs"],
            serde_json::json!(released),
            "{case}"
        );
        let notified = events(&fx, &fx.id, "owner_notified");
        assert_eq!(notified.len(), 1, "{case}");
        assert_eq!(notified[0]["producer"], "autopilot_escalate");
        assert_eq!(fx.herdr_log("notify.log").lines().count(), 1, "{case}");
        assert!(events(&fx, &fx.id, "infra_hold").is_empty(), "{case}");
        assert!(fx.task().infra_wait.is_none(), "{case}");
        if case == "dirty target" {
            assert_eq!(
                std::fs::read(checkout.join("README.md")).unwrap(),
                b"owner edit\n"
            );
        }
    }
}
