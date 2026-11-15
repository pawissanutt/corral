use super::*;
use corral_core::Store;
use serde_json::json;

#[test]
fn failed_plan_runs_and_unsubmitted_designs_never_await_acceptance() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut ctx = Ctx::from_store(Store::init(dir.path())?);
    ctx.store.config.review.reviewer = Some("none".into());
    let task = ctx.store.create("Design", None, Status::Backlog, "")?;
    let mut head = ctx.store.head(&task.id)?;
    head.task.design = true;
    for status in [Status::Backlog, Status::Idea] {
        head.task.status = status;
        assert!(is_plan(&head));
        assert_eq!(state(&ctx, &head), PlanState::Drafting);
    }
    head.last_plan_run = Some(Box::new(serde_json::from_value(json!({
        "id": "D1", "profile": "planner", "harness": "omo",
        "model": null, "vendor": null, "effort": null,
        "spec_revision": "spec", "started": "2026-11-15T00:00:00Z",
        "ended": "2026-11-15T00:01:00Z", "herdr": null,
        "output_dir": "/unused", "submitted": null, "rounds": 0,
        "state": "failed", "pending_reply": "planner exited unsuccessfully"
    }))?));
    for status in [Status::Backlog, Status::Idea, Status::NeedsMe] {
        head.task.status = status;
        assert_eq!(state(&ctx, &head), PlanState::PlanFailed);
    }
    for (run_state, expected) in [
        (PlanRunState::Cancelled, PlanState::Cancelled),
        (PlanRunState::Planning, PlanState::Drafting),
        (PlanRunState::AwaitingOwner, PlanState::Drafting),
    ] {
        head.last_plan_run.as_mut().unwrap().state = run_state;
        assert_eq!(state(&ctx, &head), expected);
    }

    // A manually submitted document can go directly to the owner only when policy
    // requires no independent reviewer. Merely having a document is not enough.
    let mut run = head.last_plan_run.take().unwrap();
    head.last_plan = Some(Box::new(serde_json::from_value(json!({
        "id": "P1", "spec_revision": "spec",
        "artifact": {
            "run": "D1", "kind": "plan", "title": "Plan", "path": "plan.md",
            "source": "/unused", "sha256": "snapshot", "bytes": 0,
            "added": "2026-11-15T00:01:00Z"
        },
        "document": {
            "goal": "Design", "non_goals": [], "decisions": [],
            "affected_paths": [], "contracts": [], "dependencies": [],
            "acceptance": [], "verification": [], "owner_decisions": []
        }
    }))?));
    assert_eq!(state(&ctx, &head), PlanState::Awaiting);
    ctx.store.config.review.reviewer = Some("reviewer".into());
    assert_eq!(state(&ctx, &head), PlanState::Reviewing);
    let review = serde_json::from_value(json!({
        "id": "PR1", "reviewer": "reviewer", "policy_digest": "policy",
        "verdict": "approve", "at": "2026-11-15T00:02:00Z",
        "herdr": null, "pending_reply": null
    }))?;
    head.last_plan.as_mut().unwrap().current_review = Some(review);
    assert_eq!(state(&ctx, &head), PlanState::Awaiting);
    head.task.status = Status::Review;
    assert_eq!(state(&ctx, &head), PlanState::Reviewing);
    head.last_plan
        .as_mut()
        .unwrap()
        .current_review
        .as_mut()
        .unwrap()
        .verdict = Some(Verdict::Changes);
    assert_eq!(state(&ctx, &head), PlanState::Drafting);

    let plan = head.last_plan.as_mut().unwrap();
    plan.run = Some("D1".into());
    plan.current_review.as_mut().unwrap().verdict = Some(Verdict::Approve);
    run.submitted = Some("P1".into());
    run.state = PlanRunState::AwaitingOwner;
    head.last_plan_run = Some(run);
    assert_eq!(state(&ctx, &head), PlanState::Awaiting);
    head.last_plan_run.as_mut().unwrap().state = PlanRunState::Failed;
    assert_eq!(state(&ctx, &head), PlanState::PlanFailed);
    Ok(())
}
