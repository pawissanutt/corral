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

    for kind in [Kind::Gate, Kind::Reviewer] {
        let mut fx = Fixture::new(1);
        fx.ctx.store.config.dispatch.paused = false;
        fx.ctx.store.config.review.reviewer = Some("rev".into());
        fx.ctx.store.save_config().unwrap();
        let id = fx.task();
        fx.ctx
            .store
            .update(&id, |task| {
                task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
                task.cowork = Some(serde_json::from_value(json!({
                    "session":"A-1", "scratch":"fix", "run":"R1",
                    "review":"required", "owner_confirmed":true
                }))?);
                Ok(())
            })
            .unwrap();
        let action = match kind {
            Kind::Gate => {
                crate::gate::gate_start_automatic(&fx.ctx, &id).unwrap();
                corral_core::OperatingActionKind::Gate
            }
            Kind::Reviewer => {
                crate::reviewer::review_run_automatic(&fx.ctx, &id, None).unwrap();
                corral_core::OperatingActionKind::Review
            }
        };
        let attempt = |task: &corral_core::Task| {
            let run = task.current_run().unwrap();
            match kind {
                Kind::Gate => run
                    .gate
                    .as_ref()
                    .unwrap()
                    .identity
                    .as_ref()
                    .unwrap()
                    .attempt
                    .clone(),
                Kind::Reviewer => run.review_runs.last().unwrap().id.clone(),
            }
        };
        let before = fx.ctx.store.get(&id).unwrap();
        let authority =
            crate::operating::load(&fx.ctx, &id, "R1", action, &attempt(&before)).unwrap();
        assert!(!crate::operating::is_automatic(&authority));
        assert!(crate::operating::automatic_driver(&authority));
        crate::hold::hold(&fx.ctx, &id, None).unwrap();
        fx.ctx.store.config.automation.paused = true;
        fx.ctx.store.save_config().unwrap();
        crate::hold::release(&fx.ctx, &id).unwrap();
        crate::hold::resume_gates(&fx.ctx).unwrap();
        assert_eq!(fx.created(), 1, "paused co-work must not restart {kind:?}");

        fx.ctx.store.config.automation.paused = false;
        fx.ctx.store.save_config().unwrap();
        fx.ctx
            .store
            .update(&id, |task| {
                task.cowork.as_mut().unwrap().owner_confirmed = false;
                Ok(())
            })
            .unwrap();
        crate::hold::resume_gates(&fx.ctx).unwrap();
        assert_eq!(fx.created(), 1, "revoked co-work must not restart {kind:?}");

        fx.ctx
            .store
            .update(&id, |task| {
                task.cowork.as_mut().unwrap().owner_confirmed = true;
                Ok(())
            })
            .unwrap();
        crate::hold::resume_gates(&fx.ctx).unwrap();
        assert_eq!(fx.created(), 2, "authorized co-work resumes {kind:?}");
        let resumed = fx.ctx.store.get(&id).unwrap();
        assert_ne!(attempt(&resumed), attempt(&before));
        let next = crate::operating::load(&fx.ctx, &id, "R1", action, &attempt(&resumed)).unwrap();
        assert_eq!(
            next.origin, authority.origin,
            "carry the same co-work instruction"
        );
        assert_eq!(resumed.rounds, 0);
        assert_eq!(resumed.current_run().unwrap().failures, 0);
    }
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
