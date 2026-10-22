use super::*;

const PLAN: &str = "---\ngoal: Make the widget spin\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nImplementation details.\n";

/// A plan-first task reviewed by Design Review, whose preferred model may be unavailable.
fn design_review(enabled: Vec<String>) -> Fx {
    let fx = Fx::new(Setup {
        enabled,
        dispatched: false,
        configure: Box::new(|store| {
            store.config.profiles.insert(
                "design-review".into(),
                profile(
                    vec![
                        candidate("design-sol", "openai", "owner-openai", "high"),
                        candidate("design-opus", "anthropic", "owner-anthropic", "low"),
                    ],
                    ProfileRole::PlanReview,
                ),
            );
            store.config.review.reviewer = Some("design-review".into());
            store.config.review.plan_required = Some(true);
        }),
        ..Setup::default()
    });
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    fx
}

#[test]
fn design_review_opens_on_the_next_approved_candidate() {
    // Given Design Review preferring a model that is not enabled locally.
    let fx = design_review(vec![sol().model, "anthropic/design-opus-fixture".into()]);
    // When the plan review session opens.
    corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap();
    // Then it launches the approved alternative of the same profile.
    let launched = fx.launched();
    assert!(
        launched.contains("anthropic/design-opus-fixture"),
        "{launched}"
    );
    assert!(
        !launched.contains("openai/design-sol-fixture"),
        "{launched}"
    );
    let switched = fx.events("availability_switched");
    assert_eq!(switched.len(), 1);
    assert_eq!(switched[0]["role"], "plan_review");
    assert_eq!(switched[0]["profile"], "design-review");
}

#[test]
fn design_review_with_every_candidate_unavailable_does_not_open() {
    // Given no approved Design Review candidate enabled locally.
    let fx = design_review(vec![sol().model]);
    // When the plan review is requested.
    let error = format!(
        "{:#}",
        corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap_err()
    );
    // Then the attempted candidates are reported and no session launches.
    assert!(error.contains("cannot start"), "{error}");
    assert!(error.contains("design-opus"), "{error}");
    assert_eq!(fx.launched(), "");
}

/// Cross-family plan review whose preferred Anthropic reviewer is unavailable and whose
/// approved Google alternative is the only reviewer model enabled locally.
fn cross_family_plan(planner: Option<&str>) -> Fx {
    let fx = Fx::new(Setup {
        enabled: vec![sol().model, "google/review-gemini-fixture".into()],
        dispatched: false,
        configure: Box::new(|store| {
            store.config.profiles.insert(
                "review-general".into(),
                profile(
                    vec![
                        opus(),
                        candidate("review-gemini", "google", "owner-google", "high"),
                    ],
                    ProfileRole::PlanReview,
                ),
            );
            store.config.review.reviewer = Some("review-general".into());
            store.config.review.require_cross_family = Some(true);
        }),
        ..Setup::default()
    });
    if planner.is_none() {
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.profile = Some("general".into());
                Ok(())
            })
            .unwrap();
    }
    if let Some(vendor) = planner {
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.plan_runs.push(corral_core::PlanRun {
                    execution: None,
                    route: None,
                    delivery: Default::default(),
                    inbox: Default::default(),
                    id: "D1".into(),
                    profile: "planner".into(),
                    harness: "omo".into(),
                    model: Some(format!("{vendor}/planner-fixture")),
                    vendor: Some(vendor.into()),
                    effort: None,
                    spec_revision: corral_ops::plan_spec_revision(task)?,
                    started: fx.at(0),
                    ended: None,
                    herdr: None,
                    output_dir: fx.ctx.store.root().join("plan-output"),
                    submitted: None,
                    rounds: 0,
                    state: corral_core::PlanRunState::Planning,
                    pending_reply: None,
                });
                Ok(())
            })
            .unwrap();
    }
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    fx
}

#[test]
fn a_recorded_planner_is_judged_against_the_review_candidate_that_starts() {
    // Given an Anthropic planner and an unavailable Anthropic preferred reviewer.
    let fx = cross_family_plan(Some("anthropic"));
    // When the plan review opens.
    corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap();
    // Then the independent, approved Google reviewer launches.
    let launched = fx.launched();
    assert!(
        launched.contains("google/review-gemini-fixture"),
        "{launched}"
    );
    assert!(!launched.contains("anthropic/opus-fixture"), "{launched}");
}

#[test]
fn a_manually_submitted_plan_uses_the_policy_author_for_its_fallback() {
    // Given a plan submitted without a planning run, authored by the task's OpenAI route.
    let fx = cross_family_plan(None);
    // When the plan review opens.
    corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap();
    // Then the fallback is judged against that author and launches.
    let launched = fx.launched();
    assert!(
        launched.contains("google/review-gemini-fixture"),
        "{launched}"
    );
}

#[test]
fn a_same_family_fallback_is_still_refused() {
    // Given an Anthropic planner whose only available approved reviewer is Anthropic too.
    let fx = cross_family_plan(Some("google"));
    // When the plan review is requested.
    let error = format!(
        "{:#}",
        corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap_err()
    );
    // Then independence is not downgraded to obtain availability.
    assert!(
        error.contains("cannot start") || error.contains("vendor"),
        "{error}"
    );
    assert_eq!(fx.launched(), "");
}

#[test]
fn a_same_family_pm_is_refused_despite_an_unused_independent_fallback() {
    // Given an OpenAI-authored plan and a PM on the same OpenAI model, whose profile also
    // lists an approved, available Anthropic alternative that the PM session does not use.
    let fx = Fx::new(Setup {
        dispatched: false,
        enabled: vec![sol().model, opus().model],
        configure: Box::new(|store| {
            let mut pm = store.config.profiles["general"].clone();
            pm.metadata.as_mut().unwrap().eligibility.roles =
                [ProfileRole::Pm, ProfileRole::PlanReview].into();
            store.config.profiles.insert("coordinator".into(), pm);
            store.config.pm.agent = "coordinator".into();
            store.config.review.reviewer = Some("pm".into());
            store.config.review.require_cross_family = Some(true);
        }),
        ..Setup::default()
    });
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.profile = Some("general".into());
            Ok(())
        })
        .unwrap();
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    // When the PM is asked to review the plan.
    let result = corral_ops::plan_review(&fx.ctx, &fx.id, None);
    // Then its actual same-family model is refused and nothing launches.
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("vendor"), "{error}");
    assert_eq!(fx.launched(), "");
}
