use super::*;

#[test]
fn deleted_plan_caches_preserve_show_review_and_member_dispatch_gate() {
    let (tmp, ctx, id, path) = fixture();
    let submitted = PLAN.replace("dependencies: []", "dependencies:\n  - task: One\n    write_scope: [one.rs]\n    depends_on: []\n  - task: Two\n    write_scope: [two.rs]\n    depends_on: []");
    std::fs::write(&path, &submitted).unwrap();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    std::fs::write(&path, "edited cache, not submitted bytes").unwrap();
    corral_core::plan_policy::fresh(&ctx.store, &ctx.store.get(&id).unwrap(), &plan).unwrap();
    std::fs::remove_file(&path).unwrap();
    let snapshot = ctx.store.dir().join(&plan.artifact.path);
    if snapshot.exists() {
        std::fs::remove_file(snapshot).unwrap();
    }
    // The show command reads this same stored record, not the removed source.
    let shown = ctx.store.get(&id).unwrap();
    assert_eq!(shown.plans[0].document.goal, "Make the widget spin");
    assert_eq!(
        corral_ops::artifact_read(&ctx, &plan.artifact).unwrap(),
        submitted.as_bytes()
    );
    corral_core::plan_policy::fresh(&ctx.store, &shown, &shown.plans[0]).unwrap();
    // Tell the fake engine to expose the launched review pane immediately, rather than
    // exercising the unrelated launch-observation timeout path.
    std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
    let review = corral_ops::plan_review(&ctx, &id, Some("reviewer")).unwrap();
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: Some(&review.id),
            verdict: Verdict::Approve,
            notes: "Approved",
        },
    )
    .unwrap();
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Owner accepts",
        },
    )
    .unwrap();
    let parent = ctx.store.get(&id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert_eq!(parent.children.len(), 2);
    let member = ctx.store.get(&parent.children[0]).unwrap();
    assert_eq!(member.plan, Some(format!("{id}@{}", plan.id)));
    // Member dispatch validates the accepted parent's immutable design, not its source cache.
    corral_core::plan_policy::validate_owner_resolution(&ctx.store, &ctx.store.config, &member)
        .unwrap();
}
