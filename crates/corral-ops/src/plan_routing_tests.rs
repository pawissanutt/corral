use super::*;
use corral_core::ProfileChoice;

fn preview_fixture() -> (tempfile::TempDir, Ctx, String) {
    let (tmp, mut ctx, id, _) = fixture();
    ctx.store
        .update(&id, |task| {
            task.profile = Some("gpt-sol".into());
            Ok(())
        })
        .unwrap();
    let agent = tmp.path().join("agent");
    std::fs::create_dir(&agent).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"enabledModels":["fixture-planner"]}"#,
    )
    .unwrap();
    ctx.store.config.sync.omo_agent_dir = Some(agent);
    ctx.store.save_config().unwrap();
    (tmp, ctx, id)
}

#[test]
fn plan_reviewer_preview_matches_actual_review_for_every_policy_mode() {
    for (policy, expected, source) in [
        ("gpt-review", "gpt-review", "workspace_review_config"),
        ("cross-vendor", "gpt-review", "cross_vendor_map"),
        ("pm", "pm", "pm"),
        ("none", "gpt-review", "independent_profile"),
    ] {
        // Given a submitted plan and distinct planner/reviewer effective models.
        let (tmp, mut ctx, id) = preview_fixture();
        ctx.store.config.review.reviewer = Some(policy.into());
        ctx.store.config.pm.agent = "gpt-review".into();
        if policy == "pm" {
            std::fs::write(tmp.path().join("engine/planner-live"), "").unwrap();
            std::fs::write(
                ctx.store.state_dir().join("pm.json"),
                r#"{"pane_id":"w1:p1"}"#,
            )
            .unwrap();
        }
        if policy == "cross-vendor" {
            ctx.store
                .config
                .profiles
                .get_mut("gpt-review")
                .unwrap()
                .vendor = Some("anthropic".into());
            ctx.store
                .config
                .routing
                .reviewer_by_vendor
                .insert("anthropic".into(), "gpt-review".into());
        }
        ctx.store.save_config().unwrap();
        let preview = crate::route(&ctx, &id, &ProfileChoice::default())
            .unwrap()
            .review;
        assert_eq!(preview.planner.unwrap().profile, "gpt-sol");
        let mapping = preview.plan_reviewer.unwrap();
        assert_eq!(mapping.profile.as_deref(), Some("gpt-review"));
        assert_eq!(mapping.model.as_deref(), Some("fixture-reviewer"));
        assert_eq!(mapping.source, source);
        // When starting the plan reviewer through the real plan-review service.
        let review = crate::plan_review(&ctx, &id, None).unwrap();
        // Then the previewed selection, not an unrelated fallback, actually launches.
        assert_eq!(review.reviewer, expected);
        assert!(review.herdr.is_some());
        assert_eq!(
            ctx.store.get(&id).unwrap().plans[0].reviews[0].reviewer,
            expected
        );
    }
}

#[test]
fn task_override_and_clear_change_both_plan_preview_and_launch() {
    for clear in [false, true] {
        // Given different workspace and task reviewers.
        let (_tmp, mut ctx, id) = preview_fixture();
        let mut other = ctx.store.config.profiles["gpt-review"].clone();
        other.model = Some("other-review-model".into());
        ctx.store
            .config
            .profiles
            .insert("other-review".into(), other);
        ctx.store.save_config().unwrap();
        crate::reviewer_set(&ctx, &id, Some("other-review")).unwrap();
        if clear {
            crate::reviewer_set(&ctx, &id, None).unwrap();
        }
        let expected = if clear { "gpt-review" } else { "other-review" };
        // When previewing and starting plan review.
        let mapping = crate::route(&ctx, &id, &ProfileChoice::default())
            .unwrap()
            .review
            .plan_reviewer
            .unwrap();
        let review = crate::plan_review(&ctx, &id, None).unwrap();
        // Then the task override wins until explicitly cleared.
        assert_eq!(mapping.profile.as_deref(), Some(expected));
        assert_eq!(review.reviewer, expected);
        assert_eq!(
            mapping.source,
            if clear {
                "workspace_review_config"
            } else {
                "task_override"
            }
        );
    }
}

#[test]
fn same_planner_override_warns_and_actual_plan_review_rejects_it() {
    // Given a task override pointing back to its planner.
    let (_tmp, ctx, id) = preview_fixture();
    crate::reviewer_set(&ctx, &id, Some("gpt-sol")).unwrap();
    // When previewing a selection that cannot satisfy independence.
    let mapping = crate::route(&ctx, &id, &ProfileChoice::default())
        .unwrap()
        .review
        .plan_reviewer
        .unwrap();
    // Then its identity stays visible with warnings, and actual launch refuses it.
    assert_eq!(mapping.profile.as_deref(), Some("gpt-sol"));
    assert_eq!(mapping.model.as_deref(), Some("fixture-planner"));
    assert!(!mapping.warnings.is_empty());
    assert!(crate::plan_review(&ctx, &id, None).is_err());
    assert!(ctx.store.get(&id).unwrap().plans[0].reviews.is_empty());
}
