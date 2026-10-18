use super::PLAN;
use corral_core::{Status, Store, Verdict};
use corral_ops::{PlanVerdict, approved_plan, plan_submit, plan_verdict};

#[test]
fn owner_resolved_single_task_plan_allows_fresh_scoped_implementation() {
    use corral_core::{OperatingActionKind, OperatingAuthority, OperatingSelection};
    // Given the existing explicit owner-verdict flow, with no planner or agent submission.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| {
            doc.set_str("projects.app.path", ".")?;
            doc.set_str("review.plan_required", "true")?;
            doc.set_str("review.reviewer", "none")
        })
        .unwrap();
    let task = store
        .create(
            "Storage",
            Some("app"),
            Status::Ready,
            "Implement storage.\n",
        )
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    let path = root.path().join("plan.md");
    std::fs::write(
        &path,
        PLAN.replace("owner_decisions: []", "owner_decisions: [Choose storage]"),
    )
    .unwrap();
    let plan = plan_submit(&ctx, &task.id, &path).unwrap();
    let review = plan_verdict(
        &ctx,
        &task.id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Resolved: Choose storage => SQLite",
        },
    )
    .unwrap();
    assert_eq!(review.verdict, Some(Verdict::Approve));
    let accepted = ctx.store.get(&task.id).unwrap();
    assert!(approved_plan(&ctx, &accepted).unwrap().is_some());
    let preview = ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: std::collections::BTreeSet::from([task.id.clone()]),
            authority: OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T04:00:00Z".parse().unwrap())
        .unwrap();
    // When a fresh consumer resolves the explicitly confirmed task scope.
    let reopened = Store::open(root.path()).unwrap();
    let effective = reopened.effective_operating_policy().unwrap();
    // Then the durable owner answer permits dispatch without deleting its immutable question.
    assert!(
        effective.allows(&task.id, OperatingActionKind::Dispatch),
        "{:?}",
        effective.tasks[&task.id].refusals
    );
    assert_eq!(
        reopened.get(&task.id).unwrap().plans[0]
            .document
            .owner_decisions,
        ["Choose storage"]
    );
}
