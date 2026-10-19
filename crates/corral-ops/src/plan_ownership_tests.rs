use super::*;

#[test]
fn sync_releases_inventory_before_plan_review_and_revision() {
    // Given a submitted plan with automatic independent review authority.
    let (tmp, ctx, id, _) = fixture();

    // When sync consumes the inventory and advances plan review.
    crate::sync_at(&ctx, Timestamp::now()).unwrap();
    let task = ctx.store.get(&id).unwrap();
    let plan = task.plans.last().unwrap();
    let attempt = plan.reviews.last().unwrap();
    assert!(attempt.herdr.is_some());
    assert!(attempt.pending_reply.is_none());
    crate::plan_verdict(
        &ctx,
        &id,
        &crate::PlanVerdict {
            plan: &plan.id,
            attempt: Some(&attempt.id),
            verdict: Verdict::Changes,
            notes: "Revise the plan",
        },
    )
    .unwrap();
    crate::sync_at(&ctx, Timestamp::now()).unwrap();

    // Then the planner revision also acquires exclusive ownership in that tick.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plan_runs[0].rounds, 1);
    assert_eq!(task.status, Status::Running);
    assert!(tmp.path().join("engine/planner-live").exists());
    assert!(ctx.store.try_lock("session-ownership").unwrap().is_some());
}
