use super::*;
use corral_core::{OperatingActionKind, OperatingAuthority, OperatingSelection, Verdict};

#[test]
fn held_review_does_not_launch_verification_while_independent_work_continues() {
    // Given a completed scoped candidate and an independent Ready task.
    let mut fx = Fx::new();
    let held = fx.task("held verification", Status::Ready);
    fx.start(&held.id);
    fx.done(&held.id, "held.txt");
    let independent = fx.task("independent", Status::Ready);
    fx.grant(&[&held.id, &independent.id]);
    fx.ctx
        .store
        .update(&held.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let before = fx.records("submissions.jsonl").len();

    // When fresh sync processes both tasks with capacity available.
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();

    // Then only independent work starts; the held candidate has no new tools.
    let retained = fx.ctx.store.get(&held.id).unwrap();
    assert_eq!(retained.status, Status::Review);
    assert!(retained.dispatch_hold);
    let run = retained.current_run().unwrap();
    assert!(run.gate.is_none());
    assert!(run.review.is_none());
    assert_eq!(fx.records("submissions.jsonl").len(), before + 1);
    assert_eq!(
        fx.ctx.store.get(&independent.id).unwrap().status,
        Status::Running
    );
    assert!(
        !fx.ctx
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&held.id, OperatingActionKind::Gate)
    );
}

#[test]
fn held_independently_approved_candidate_does_not_land() {
    // Given a real passing gate and independent approval under a finite grant.
    let mut fx = Fx::new();
    let held = fx.task("held landing", Status::Ready);
    fx.start(&held.id);
    fx.done(&held.id, "held-landing.txt");
    corral_ops::gate_start(&fx.ctx, &held.id).unwrap();
    corral_ops::gate_collect(&fx.ctx, &held.id)
        .unwrap()
        .unwrap();
    let review = corral_ops::review_run(&fx.ctx, &held.id, Some("rev")).unwrap();
    corral_ops::verdict_bound(
        &fx.ctx,
        &held.id,
        Verdict::Approve,
        "Independent approval",
        Some(&review.identity.unwrap().attempt),
    )
    .unwrap();
    fx.grant(&[&held.id]);
    fx.ctx
        .store
        .update(&held.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let before = fx.ctx.store.get(&held.id).unwrap();
    let submitted = fx.records("submissions.jsonl").len();

    // When sync observes the verified candidate with an owner hold.
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();

    // Then receipts survive without new submissions or a Git landing.
    let retained = fx.ctx.store.get(&held.id).unwrap();
    assert_eq!(retained.status, Status::Review);
    assert!(retained.dispatch_hold);
    assert_eq!(
        retained.current_run().unwrap().gate,
        before.current_run().unwrap().gate
    );
    assert_eq!(
        retained.current_run().unwrap().review,
        before.current_run().unwrap().review
    );
    assert_eq!(fx.records("submissions.jsonl").len(), submitted);
    assert!(!fx.temp.path().join("app/held-landing.txt").exists());
}

#[test]
fn held_reviewed_plan_cannot_publish_children() {
    // Given independent approval of a plan with exactly confirmed child scopes.
    let mut fx = Fx::new();
    let parent = fx.task("held plan publication", Status::Ready);
    let source = fx.ctx.store.state_dir().join("held-plan.md");
    std::fs::write(
        &source,
        "---\ngoal: Ship finite work\nnon_goals: []\ndecisions: []\naffected_paths: [first.txt, second.txt]\ncontracts: []\ndependencies:\n  - task: First\n    write_scope: [first.txt]\n    depends_on: []\n  - task: Second\n    write_scope: [second.txt]\n    depends_on: [First]\nacceptance: [{scenario: finite, expected: verified}]\nverification: [check]\nowner_decisions: []\n---\n",
    )
    .unwrap();
    let plan = corral_ops::plan_submit(&fx.ctx, &parent.id, &source).unwrap();
    let review = corral_ops::plan_review(&fx.ctx, &parent.id, Some("rev")).unwrap();
    corral_ops::plan_verdict(
        &fx.ctx,
        &parent.id,
        &corral_ops::PlanVerdict {
            plan: &plan.id,
            attempt: Some(&review.id),
            verdict: Verdict::Approve,
            notes: "Independent scope review",
        },
    )
    .unwrap();
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: [parent.id.clone()].into(),
            plan_scopes: [(parent.id.clone(), ["First".into(), "Second".into()].into())].into(),
            authority: OperatingAuthority {
                dispatch: true,
                accept: true,
                plan_accept: true,
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", now())
        .unwrap();
    fx.ctx
        .store
        .update(&parent.id, |task| {
            task.status = Status::Review;
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let submitted = fx.records("submissions.jsonl").len();

    // When the plan pilot reconciles the held reviewed parent.
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();

    // Then no automatic acceptance, hold clearance, publication or child launch occurs.
    let retained = fx.ctx.store.get(&parent.id).unwrap();
    assert_eq!(retained.status, Status::Review);
    assert!(retained.dispatch_hold);
    assert!(retained.plans.last().unwrap().approval.is_none());
    assert!(retained.children.is_empty());
    assert_eq!(fx.ctx.store.list().unwrap().len(), 1);
    assert_eq!(fx.records("submissions.jsonl").len(), submitted);
}
