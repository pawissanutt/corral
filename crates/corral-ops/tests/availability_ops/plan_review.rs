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
