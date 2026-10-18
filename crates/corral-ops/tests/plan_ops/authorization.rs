use super::*;

fn approve(ctx: &Ctx, id: &str, plan: &str) {
    let review = plan_verdict(
        ctx,
        id,
        &PlanVerdict {
            plan,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Independent review passed",
        },
    )
    .unwrap();
    assert_eq!(review.verdict, Some(Verdict::Approve));
}

#[test]
fn legacy_auto_setting_without_a_grant_records_review_but_never_accepts() {
    // Given the former automatic setting, without owner confirmation of scope.
    let (_root, mut ctx, id, path) = fixture();
    ctx.store.config.review.plan_accept = Some(corral_core::Accept::Auto);
    ctx.store.save_config().unwrap();
    let plan = pending(&ctx, &id, &path);
    // When a valid independent verdict arrives.
    approve(&ctx, &id, &plan);
    // Then collection succeeds, but the setting alone supplies no acceptance authority.
    let task = ctx.store.get(&id).unwrap();
    assert!(task.plans.last().unwrap().approval.is_none());
    assert!(task.plan.is_none());
    assert_eq!(task.status, Status::NeedsMe);
}

#[test]
fn delegated_plan_acceptance_does_not_require_the_legacy_auto_switch() {
    // Given a confirmed acceptance grant and only the built-in owner default.
    let (_root, ctx, id, path) = fixture();
    let plan = pending(&ctx, &id, &path);
    confirm_plan_authority(&ctx.store, &id);
    // When the independent reviewer approves.
    approve(&ctx, &id, &plan);
    // Then the separate authorized acceptance is durable, without coding authority.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(
        task.plans.last().unwrap().approval.as_ref().unwrap().by,
        "reviewer"
    );
    assert_eq!(task.plan, Some(format!("{id}@{plan}")));
    assert!(task.runs.is_empty());
    assert!(
        !Store::open(ctx.store.root())
            .unwrap()
            .config
            .automation
            .authority
            .accept
    );
}

#[test]
fn pause_collects_the_plan_verdict_without_acceptance_or_an_owner_hold() {
    // Given authorized work paused through a different fresh store.
    let (_root, ctx, id, path) = fixture();
    let plan = pending(&ctx, &id, &path);
    confirm_plan_authority(&ctx.store, &id);
    Store::open(ctx.store.root())
        .unwrap()
        .pause_operating_policy(true)
        .unwrap();
    // When the running reviewer produces its receipt during the pause.
    approve(&ctx, &id, &plan);
    // Then the review survives and the automatic wait does not become a NeedsMe hold.
    let task = ctx.store.get(&id).unwrap();
    assert!(task.plans.last().unwrap().approval.is_none());
    assert_eq!(task.status, Status::Review);
    assert!(task.children.is_empty());
}

#[test]
fn undelegated_plan_scopes_preserve_review_without_publishing_children() {
    // Given a task grant that never confirmed this plan's newly proposed children.
    let (_root, ctx, id, path) = fixture();
    std::fs::write(
        &path,
        PLAN.replace(
            "dependencies: []",
            "dependencies:\n  - task: First\n    write_scope: [first.rs]\n    depends_on: []\n  - task: Second\n    write_scope: [second.rs]\n    depends_on: [First]",
        ),
    )
    .unwrap();
    let plan = pending(&ctx, &id, &path);
    confirm_plan_authority(&ctx.store, &id);
    // When the independent review approves the snapshot.
    approve(&ctx, &id, &plan);
    // Then scope refusal retains that receipt and creates no implementation tasks.
    let task = ctx.store.get(&id).unwrap();
    assert!(task.plans.last().unwrap().approval.is_none());
    assert!(task.children.is_empty());
    assert_eq!(ctx.store.list().unwrap().len(), 1);
}

#[test]
fn coordinating_pm_review_cannot_supply_automatic_plan_acceptance() {
    // Given a PM review receipt under its exact configured reviewer policy.
    let (_root, mut ctx, id, path) = fixture();
    ctx.store.config.review.reviewer = Some("pm".into());
    ctx.store.save_config().unwrap();
    let plan = pending(&ctx, &id, &path);
    ctx.store
        .update(&id, |task| {
            task.plans
                .last_mut()
                .unwrap()
                .reviews
                .last_mut()
                .unwrap()
                .reviewer = "pm".into();
            Ok(())
        })
        .unwrap();
    confirm_plan_authority(&ctx.store, &id);
    // When the PM approves its review attempt.
    approve(&ctx, &id, &plan);
    // Then even a scoped grant cannot turn coordination into independent acceptance.
    assert!(
        ctx.store
            .get(&id)
            .unwrap()
            .plans
            .last()
            .unwrap()
            .approval
            .is_none()
    );
}
