use super::*;

fn register(ctx: &mut Ctx, reviewer: &str) {
    ctx.store.config.agents.insert(
        reviewer.into(),
        corral_core::AgentCmd {
            command: "true".into(),
        },
    );
    ctx.store.config.review.reviewer = Some(reviewer.into());
    ctx.store.save_config().unwrap();
}

/// Start a fresh independent review attempt on the current plan and approve it.
fn rereview(ctx: &Ctx, id: &str, plan: &str, reviewer: &str) {
    let digest =
        crate::plan::policy_digest(&ctx.store.config, &ctx.store.get(id).unwrap()).unwrap();
    let attempt = format!(
        "PR{}",
        ctx.store
            .get(id)
            .unwrap()
            .plans
            .last()
            .unwrap()
            .reviews
            .len()
            + 1
    );
    ctx.store
        .update(id, |task| {
            let record = task.plans.last_mut().unwrap();
            record.reviews.push(corral_core::PlanReview {
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                route: None,
                id: attempt.clone(),
                reviewer: reviewer.into(),
                policy_digest: digest,
                verdict: None,
                notes: None,
                at: "2026-10-27T08:00:00Z".parse().unwrap(),
                herdr: None,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
    crate::plan_verdict(
        ctx,
        id,
        &crate::PlanVerdict {
            plan,
            attempt: Some(&attempt),
            verdict: corral_core::Verdict::Approve,
            notes: "Independent re-review passed",
        },
    )
    .unwrap();
}

#[test]
fn stale_approval_of_a_decomposed_done_design_is_refreshed_without_reopening_it() {
    // Given a Done design task whose accepted plan published two children.
    let (_dir, mut ctx, id) = fixture(Vec::new());
    register(&mut ctx, "reviewer");
    let document = ctx.store.get(&id).unwrap().plans[0].document.clone();
    ctx.store
        .update(&id, |task| {
            task.plans.clear();
            Ok(())
        })
        .unwrap();
    let plan = submit_reviewed_plan(
        &ctx,
        &id,
        &document,
        vec![scope("a", &[]), scope("b", &["a"])],
    );
    owner_accept(&ctx, &id, &plan).unwrap();
    let published = ctx.store.get(&id).unwrap();
    assert_eq!(published.status, Status::Done);
    assert_eq!(published.children, ["T-002", "T-003"]);
    let board = ctx.store.list().unwrap().len();
    let approval = |ctx: &Ctx| {
        let parent = ctx.store.get(&id).unwrap();
        corral_core::plan_policy::validate_approval(
            &ctx.store,
            &ctx.store.config,
            &parent,
            parent.plans.last().unwrap(),
        )
    };
    approval(&ctx).unwrap();

    // When a reviewer policy change makes that approval stale for the children.
    register(&mut ctx, "reviewer-2");
    let error = approval(&ctx).unwrap_err();
    assert!(format!("{error:#}").contains("stale plan approval"));

    // And the plan is re-reviewed, then re-accepted by the owner.
    rereview(&ctx, &id, &plan, "reviewer-2");
    let after_review = ctx.store.get(&id).unwrap();
    assert_eq!(after_review.status, Status::Done);
    let (review, refreshed) = crate::plan_verdict_report(
        &ctx,
        &id,
        &crate::PlanVerdict {
            plan: &plan,
            attempt: None,
            verdict: corral_core::Verdict::Approve,
            notes: "Owner re-accepts",
        },
    )
    .unwrap();

    // Then the task stays Done and unqueued, and the verdict names the children.
    assert_eq!(review.verdict, Some(corral_core::Verdict::Approve));
    assert_eq!(refreshed, ["T-002", "T-003"]);
    let parent = ctx.store.get(&id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert!(!parent.queued && parent.dispatch_plan.is_none());
    assert_eq!(parent.children, ["T-002", "T-003"]);
    assert_eq!(ctx.store.list().unwrap().len(), board);
    // And the children no longer hit the stale-approval refusal or wait on a reopened parent.
    approval(&ctx).unwrap();
    let child = ctx.store.get("T-002").unwrap();
    assert!(
        crate::queue::dependency_check(&ctx.store, &child)
            .unwrap()
            .is_none()
    );
}
