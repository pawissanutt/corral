use super::*;

#[test]
fn hold_cancels_gate_without_failure_or_worker_changes_and_release_requeues() {
    let fx = Fixture::new(2);
    let id = fx.task();
    let gate = crate::gate::gate_start(&fx.ctx, &id).unwrap();
    let before = fx.ctx.store.get(&id).unwrap();
    crate::hold::hold(&fx.ctx, &id, Some("P0 first")).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, before.status);
    let run = task.current_run().unwrap();
    assert_eq!(run.herdr, before.current_run().unwrap().herdr);
    assert_eq!(run.gate.as_ref().unwrap().result, GateResult::Cancelled);
    assert_eq!(run.failures, 0);
    assert_eq!(task.rounds, 0);
    assert!(!run.gate.as_ref().unwrap().holds_slot());
    assert!(!fx.tmp.path().join(gate.pane.unwrap()).exists());
    let identity = gate.identity.unwrap();
    let row = fx
        .ctx
        .store
        .gate_attempt(&id, "R1", &identity.attempt)
        .unwrap()
        .unwrap();
    assert_eq!(row.result.as_deref(), Some("cancelled"));
    assert!(row.ended.is_some());
    assert!(
        crate::gate::gate_start(&fx.ctx, &id)
            .unwrap_err()
            .to_string()
            .contains("Held")
    );
    crate::hold::release(&fx.ctx, &id).unwrap();
    crate::hold::resume_gates(&fx.ctx).unwrap();
    let next = fx
        .ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .gate
        .clone()
        .unwrap();
    assert_ne!(next.identity.unwrap().attempt, identity.attempt);
    assert_eq!(fx.ctx.store.get(&id).unwrap().rounds, 0);
}

#[test]
fn hold_cancels_reviewer_without_a_verdict_or_repair_round() {
    let fx = Fixture::new(1);
    let id = fx.task();
    let record = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    crate::hold::hold(&fx.ctx, &id, None).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert!(run.review.is_none());
    let child = run.review_runs.last().unwrap();
    assert_eq!(child.outcome, Some(corral_core::RunOutcome::Cancelled));
    assert!(child.review.as_ref().unwrap().verdict.is_none());
    assert!(child.ended.is_some());
    assert_eq!(run.failures, 0);
    assert_eq!(task.rounds, 0);
    assert!(!fx.tmp.path().join(record.pane.unwrap()).exists());
    assert!(
        crate::reviewer::verdict_bound(&fx.ctx, &id, Verdict::Changes, "late", Some(&child.id))
            .is_err()
    );
    crate::hold::release(&fx.ctx, &id).unwrap();
    crate::hold::resume_gates(&fx.ctx).unwrap();
    assert!(
        fx.ctx
            .store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .is_some()
    );
    assert_eq!(fx.ctx.store.get(&id).unwrap().rounds, 0);
}

#[test]
fn hold_cancels_plan_reviews_and_removes_pending_slot_facts() {
    let fx = Fixture::new(2);
    let id = fx.task();
    fx.mark("w1:plan-review");
    let plan: corral_core::PlanRecord = serde_json::from_value(json!({
        "id":"P1", "spec_revision":"fixture", "document":{
            "goal":"fixture", "non_goals":[], "decisions":[], "affected_paths":[],
            "contracts":[], "dependencies":[], "acceptance":[], "verification":[], "owner_decisions":[]
        },
        "artifact":{"run":"R1","kind":"plan","title":"plan","path":"fixture","source":"fixture","sha256":"fixture","bytes":0,"added":"2026-10-15T00:00:00Z"},
        "reviews":[{"id":"P1-PR1","reviewer":"rev","policy_digest":"fixture","at":"2026-10-15T00:00:00Z", "herdr":{"workspace_id":"w1","pane_id":"w1:plan-review", "socket":fx.ctx.herdr.socket_path().unwrap()}}]
    })).unwrap();
    fx.ctx
        .store
        .update(&id, |t| {
            t.plans.push(plan);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        fx.ctx
            .store
            .head(&id)
            .unwrap()
            .last_plan
            .as_ref()
            .unwrap()
            .facts
            .pending_children,
        1
    );
    crate::hold::hold(&fx.ctx, &id, None).unwrap();
    let head = fx.ctx.store.head(&id).unwrap();
    assert_eq!(head.last_plan.as_ref().unwrap().facts.pending_children, 0);
    assert!(head.pending_plan_reviewers().unwrap().is_empty());
    let task = fx.ctx.store.get(&id).unwrap();
    let review = task.plans.last().unwrap().reviews.last().unwrap();
    assert!(review.cancelled && review.verdict.is_none());
    assert_eq!(task.rounds, 0);
    assert_eq!(
        crate::SlotUsage::from_tasks(&[task], &fx.ctx.store.config.limits).reviews,
        0
    );
    assert!(!fx.tmp.path().join("w1:plan-review").exists());
}

#[test]
fn admission_orders_mixed_waits_by_priority_then_age_and_ignores_held() {
    let fx = Fixture::new(1);
    let older = fx.task();
    let newer = fx.task();
    fx.ctx
        .store
        .update(&older, |t| {
            t.review_wait = Some(Kind::Reviewer);
            Ok(())
        })
        .unwrap();
    let wait = ReviewLaunch::reserve(&fx.ctx, &newer, Kind::Gate)
        .err()
        .unwrap();
    assert!(wait.is::<CapacityWait>());
    assert!(wait.to_string().contains(&older));
    fx.ctx
        .store
        .update(&newer, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    ReviewLaunch::reserve(&fx.ctx, &newer, Kind::Gate)
        .unwrap()
        .commit()
        .unwrap();
    crate::hold::hold(&fx.ctx, &older, None).unwrap();
    fx.ctx
        .store
        .update(&newer, |t| {
            t.priority = "P3".into();
            Ok(())
        })
        .unwrap();
    ReviewLaunch::reserve(&fx.ctx, &newer, Kind::Gate)
        .unwrap()
        .commit()
        .unwrap();
    assert!(fx.ctx.store.head(&older).unwrap().review_wait.is_none());
}

#[test]
fn optional_p0_preemption_cancels_lowest_gate_and_requeues_without_failure() {
    let mut fx = Fixture::new(2);
    let low = fx.task();
    let medium = fx.task();
    let urgent = fx.task();
    fx.ctx
        .store
        .update(&low, |t| {
            t.priority = "P3".into();
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .update(&urgent, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    crate::gate::gate_start(&fx.ctx, &low).unwrap();
    crate::gate::gate_start(&fx.ctx, &medium).unwrap();
    assert!(
        crate::gate::gate_start(&fx.ctx, &urgent).is_err(),
        "preemption defaults off"
    );
    fx.ctx.store.config.limits.preempt = Some("P0".into());
    fx.ctx.store.save_config().unwrap();
    crate::gate::gate_start(&fx.ctx, &urgent).unwrap();
    let task = fx.ctx.store.get(&low).unwrap();
    assert_eq!(
        task.current_run().unwrap().gate.as_ref().unwrap().result,
        GateResult::Cancelled
    );
    assert_eq!(task.review_wait, Some(Kind::Gate));
    assert_eq!(task.current_run().unwrap().failures, 0);
    assert_eq!(task.rounds, 0);
    assert_eq!(
        fx.ctx
            .store
            .get(&medium)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Running
    );
}
