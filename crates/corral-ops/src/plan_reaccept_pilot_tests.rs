use super::*;
use corral_core::Status;

const SPLIT_PLAN: &str = "---\ngoal: Ship widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: [{task: a, write_scope: [a/**], depends_on: [], acceptance: []}, {task: b, write_scope: [b/**], depends_on: [a], acceptance: []}]\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [check]\nowner_decisions: []\n---\n";

fn verdict(ctx: &Ctx, id: &str, attempt: Option<&str>) -> (corral_core::PlanReview, Vec<String>) {
    crate::plan_verdict_report(
        ctx,
        id,
        &crate::PlanVerdict {
            plan: "P1",
            attempt,
            verdict: Verdict::Approve,
            notes: "Approved",
        },
    )
    .unwrap()
}

fn assert_parked(ctx: &Ctx, id: &str, children: &[String]) {
    let parent = ctx.store.get(id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert!(!parent.queued && parent.dispatch_plan.is_none());
    assert_eq!(parent.children, children);
}

fn approval(ctx: &Ctx, id: &str) -> anyhow::Result<()> {
    let parent = ctx.store.get(id).unwrap();
    corral_core::plan_policy::validate_approval(
        &ctx.store,
        &ctx.store.config,
        &parent,
        parent.plans.last().unwrap(),
    )
}

#[test]
fn real_review_and_owner_reacceptance_keep_a_published_design_done() {
    for run_backed in [true, false] {
        // Given a published Done design task, with or without a plan run.
        let (_tmp, mut ctx, id, _) = fixture_with(SPLIT_PLAN);
        if !run_backed {
            ctx.store
                .update(&id, |t| {
                    t.plan_runs.clear();
                    t.plans.last_mut().unwrap().run = None;
                    Ok(())
                })
                .unwrap();
        }
        let first = crate::plan_review(&ctx, &id, None).unwrap();
        verdict(&ctx, &id, Some(&first.id));
        verdict(&ctx, &id, None);
        let children = ctx.store.get(&id).unwrap().children;
        assert_eq!(children.len(), 2, "run_backed={run_backed}");
        assert_parked(&ctx, &id, &children);
        approval(&ctx, &id).unwrap();

        // When reviewer policy changes after publication, members retain approval.
        let mut reviewer = ctx.store.config.profiles["gpt-review"].clone();
        reviewer.model = Some("fixture-reviewer-2".into());
        ctx.store
            .config
            .profiles
            .insert("gpt-review-2".into(), reviewer);
        ctx.store.config.review.reviewer = Some("gpt-review-2".into());
        ctx.store.save_config().unwrap();
        approval(&ctx, &id).unwrap();
        let child = ctx.store.get(&children[0]).unwrap();
        corral_core::plan_policy::validate_owner_resolution(&ctx.store, &ctx.store.config, &child)
            .unwrap();

        // And it is re-reviewed through the real admission path, then owner re-accepted.
        let second = crate::plan_review(&ctx, &id, None).unwrap();
        assert_eq!(second.reviewer, "gpt-review-2");
        assert_parked(&ctx, &id, &children);
        verdict(&ctx, &id, Some(&second.id));
        assert_parked(&ctx, &id, &children);
        let (owner, refreshed) = verdict(&ctx, &id, None);

        // Then it stays Done and unqueued, names the refreshed children, and they pass.
        assert_eq!(owner.verdict, Some(Verdict::Approve));
        assert_eq!(refreshed, children, "run_backed={run_backed}");
        assert_parked(&ctx, &id, &children);
        approval(&ctx, &id).unwrap();
        let child = ctx.store.get(&children[0]).unwrap();
        assert!(
            crate::queue::dependency_check(&ctx.store, &child)
                .unwrap()
                .is_none()
        );
    }
}
