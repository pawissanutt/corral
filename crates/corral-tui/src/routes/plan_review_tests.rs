use corral_core::{PlanDocument, PlanRecord, PlanReview, PlanRun, PlanRunState, Verdict};

use super::*;
use crate::board::tests::Fixture;
use crate::routes::tests::fixture;

pub(crate) fn reviewing(state: PlanRunState) -> Fixture {
    let mut fx = fixture();
    fx.ctx
        .store
        .config
        .harnesses
        .retain(|name, _| name == "generic");
    fx.ctx.store.config.agents.clear();
    fx.ctx
        .store
        .config
        .profiles
        .retain(|name, _| matches!(name.as_str(), "gpt-sol" | "sonnet"));
    fx.ctx.store.config.pm.agent = "sonnet".into();
    let harness = fx.ctx.store.config.harnesses.get_mut("generic").unwrap();
    harness.command = "false {model_args}".into();
    harness.model_flag = Some("--model".into());
    for (name, model) in [
        ("gpt-sol", "openai/planner"),
        ("sonnet", "anthropic/reviewer"),
    ] {
        fx.ctx.store.config.profiles.get_mut(name).unwrap().model = Some(model.into());
    }
    let mut artifact = fx.ctx.store.get("T-006").unwrap().artifacts[0].clone();
    artifact.kind = corral_core::ArtifactKind::Plan;
    fx.ctx
        .store
        .update("T-002", |task| {
            task.status = Status::NeedsMe;
            task.plan_required = Some(true);
            task.plan_runs.push(PlanRun {
                agent_host: None,
                execution: None,
                route: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                id: "P1".into(),
                profile: "gpt-sol".into(),
                harness: "generic".into(),
                model: Some("openai/planner".into()),
                vendor: Some("openai".into()),
                effort: None,
                spec_revision: "fixture".into(),
                started: task.updated,
                ended: None,
                herdr: None,
                output_dir: fx.ctx.store.root().join("plan-fixture"),
                submitted: Some("plan-1".into()),
                rounds: 1,
                state,
                pending_reply: None,
            });
            task.plans.push(PlanRecord {
                id: "plan-1".into(),
                artifact,
                spec_revision: "fixture".into(),
                document: PlanDocument {
                    delivery_target: None,
                    human_review: None,
                    goal: "Fixture plan".into(),
                    non_goals: Vec::new(),
                    decisions: Vec::new(),
                    affected_paths: vec!["fixture.rs".into()],
                    contracts: Vec::new(),
                    dependencies: Vec::new(),
                    acceptance: Vec::new(),
                    verification: vec!["fixture check".into()],
                    owner_decisions: Vec::new(),
                },
                reviews: vec![PlanReview {
                    agent_host: None,
                    delivery: corral_core::inbox::DeliveryMode::Legacy,
                    inbox: Default::default(),
                    route: None,
                    id: "PR1".into(),
                    reviewer: "sonnet".into(),
                    policy_digest: "recorded-policy".into(),
                    verdict: (state == PlanRunState::AwaitingOwner).then_some(Verdict::Approve),
                    notes: None,
                    at: task.updated,
                    herdr: None,
                    pending_reply: None,
                }],
                run: Some("P1".into()),
                approval: None,
            });
            Ok(())
        })
        .unwrap();
    fx
}

#[test]
fn recorded_plan_reviewer_survives_future_override_set_and_clear() {
    for state in [PlanRunState::Reviewing, PlanRunState::AwaitingOwner] {
        // Given a valid pending or approved review by the recorded Sonnet reviewer.
        let fx = reviewing(state);
        let task = fx.ctx.store.get("T-002").unwrap();
        let initial = resolve(&fx.ctx, &task);
        let initial = initial
            .recorded
            .unwrap()
            .unwrap()
            .review
            .plan_reviewer
            .unwrap();
        assert_eq!(initial.profile.as_deref(), Some("sonnet"));
        assert!(initial.warnings.is_empty(), "{initial:?}");

        // When the future reviewer is pinned to the planner and then cleared through ops.
        for choice in [Some("gpt-sol"), None] {
            let task = corral_ops::reviewer_set(&fx.ctx, "T-002", choice).unwrap();
            let route = resolve(&fx.ctx, &task);
            let next = route.next.unwrap();
            let recorded = route
                .recorded
                .unwrap()
                .unwrap()
                .review
                .plan_reviewer
                .unwrap();

            // Then the board and recorded detail keep Sonnet while next-dispatch policy changes.
            assert_eq!(route.card.label, "gpt-sol -> sonnet");
            assert!(!route.card.warning);
            assert_eq!(recorded, initial);
            assert_eq!(
                next.review.reviewer.profile.as_deref(),
                Some(choice.unwrap_or("sonnet"))
            );
            assert_eq!(
                next.review.reviewer.selection,
                choice.unwrap_or("cross-vendor")
            );
            assert_eq!(
                next.review.plan_reviewer.unwrap().profile.as_deref(),
                Some(choice.unwrap_or("sonnet"))
            );
        }
    }
}

#[test]
fn new_planner_without_recorded_review_keeps_prospective_reviewer() {
    // Given an older planner's recorded review and a replacement planner with no review.
    let fx = reviewing(PlanRunState::Reviewing);
    let mut task = fx.ctx.store.get("T-002").unwrap();
    let mut replacement = task.plan_runs[0].clone();
    replacement.id = "P2".into();
    replacement.submitted = None;
    replacement.state = PlanRunState::Planning;
    task.plan_runs.push(replacement);
    task.reviewer = Some("gpt-sol".into());
    // When resolving the new planner's route.
    let route = resolve(&fx.ctx, &task);
    // Then an unrelated older review cannot override the prospective policy.
    assert_eq!(route.card.label, "gpt-sol -> gpt-sol");
    assert_eq!(
        route
            .recorded
            .unwrap()
            .unwrap()
            .review
            .plan_reviewer
            .unwrap()
            .profile
            .as_deref(),
        Some("gpt-sol")
    );
}

#[test]
fn latest_plan_without_review_does_not_reuse_a_previous_plan_review() {
    // Given a new submission from the same planner whose previous plan was reviewed.
    let fx = reviewing(PlanRunState::Reviewing);
    let mut task = fx.ctx.store.get("T-002").unwrap();
    let mut latest = task.plans[0].clone();
    latest.id = "plan-2".into();
    latest.reviews.clear();
    task.plans.push(latest);
    task.plan_runs[0].submitted = Some("plan-2".into());
    task.reviewer = Some("gpt-sol".into());
    // When resolving the latest submission, then its prospective reviewer is used.
    let route = resolve(&fx.ctx, &task);
    assert_eq!(route.card.label, "gpt-sol -> gpt-sol");
}
