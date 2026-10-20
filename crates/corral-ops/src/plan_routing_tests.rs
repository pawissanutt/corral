use super::*;
use corral_core::ProfileChoice;

fn preview_fixture() -> (tempfile::TempDir, Ctx, String) {
    let (tmp, mut ctx, id, _) = fixture();
    // Automatic selection should exercise these fixture profiles, not built-in presets.
    ctx.store.config.agents.clear();
    ctx.store
        .config
        .profiles
        .retain(|name, _| matches!(name.as_str(), "gpt-sol" | "gpt-review"));
    ctx.store.config.pm.agent = "gpt-review".into();
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
fn auto_plan_review_routes_to_the_plan_mapping_with_a_recorded_contract() {
    let (_tmp, mut ctx, id) = preview_fixture();
    ctx.store.config.review.reviewer = Some("auto".into());
    for kind in [
        corral_core::ReviewContractKind::Plan,
        corral_core::ReviewContractKind::Implementation,
    ] {
        ctx.store
            .config
            .routing
            .review_profiles
            .insert(kind, "gpt-review".into());
    }
    ctx.store.save_config().unwrap();
    let mapping = crate::route(&ctx, &id, &ProfileChoice::default())
        .unwrap()
        .review
        .plan_reviewer
        .unwrap();
    assert_eq!(mapping.profile.as_deref(), Some("gpt-review"));
    let review = crate::plan_review(&ctx, &id, None).unwrap();
    assert_eq!(review.reviewer, "gpt-review");
    let route = review.route.expect("plan review records its route");
    assert_eq!(
        route.requirement.contract.kind,
        corral_core::ReviewContractKind::Plan
    );
    assert_eq!(
        route.requirement.capability,
        corral_core::ReviewCapability::Strong
    );
    assert_eq!(route.profile_source, "auto_mapping");
    assert_eq!(route.mapping, Some(corral_core::ReviewContractKind::Plan));
    assert_eq!(
        ctx.store.get(&id).unwrap().plans[0].reviews[0].route,
        Some(route)
    );
}

fn plan_reviewer_metadata(ctx: &mut Ctx, roles: &[corral_core::ProfileRole]) {
    let profile = ctx.store.config.profiles.get_mut("gpt-review").unwrap();
    profile.metadata = Some(corral_core::ProfileMetadata {
        display_name: "Review".into(),
        purpose: "Plan review".into(),
        guidance: Default::default(),
        instructions: "Check interface ownership".into(),
        eligibility: corral_core::ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: roles.iter().copied().collect(),
            projects: Default::default(),
        },
        revision: "r1".into(),
        evidence: None,
        candidates: vec![],
        fallback: None,
    });
    ctx.store.config.review.reviewer = Some("gpt-review".into());
    ctx.store.save_config().unwrap();
}

fn approve_plan(ctx: &Ctx, id: &str, attempt: &str) -> Result<corral_core::PlanReview> {
    let plan = ctx.store.get(id).unwrap().plans.last().unwrap().id.clone();
    crate::plan_verdict(
        ctx,
        id,
        &crate::PlanVerdict {
            plan: &plan,
            attempt: Some(attempt),
            verdict: corral_core::Verdict::Approve,
            notes: "Plan is executable",
        },
    )
}

#[test]
fn plan_reviewer_without_strong_capability_escalates_its_approval() {
    let (_tmp, mut ctx, id) = preview_fixture();
    plan_reviewer_metadata(&mut ctx, &[corral_core::ProfileRole::ImplementationReview]);
    let review = crate::plan_review(&ctx, &id, None).unwrap();
    assert_eq!(review.reviewer, "gpt-review");
    assert!(review.route.as_ref().unwrap().capability_mismatch);
    let saved = approve_plan(&ctx, &id, &review.id).unwrap();
    assert_eq!(saved.verdict, Some(corral_core::Verdict::Escalate));
}

#[test]
fn plan_reviewer_profile_revision_change_stales_the_verdict() {
    let (_tmp, mut ctx, id) = preview_fixture();
    plan_reviewer_metadata(
        &mut ctx,
        &[
            corral_core::ProfileRole::PlanReview,
            corral_core::ProfileRole::ImplementationReview,
        ],
    );
    let review = crate::plan_review(&ctx, &id, None).unwrap();
    assert!(!review.route.as_ref().unwrap().capability_mismatch);
    let plan = ctx.store.get(&id).unwrap().plans.last().unwrap().id.clone();
    let prompt = std::fs::read_to_string(
        ctx.store
            .prompts_dir()
            .join(format!("{id}-{plan}-{}-plan-review.md", review.id)),
    )
    .unwrap();
    assert!(prompt.contains("Check interface ownership"));
    ctx.store
        .config
        .profiles
        .get_mut("gpt-review")
        .and_then(|p| p.metadata.as_mut())
        .unwrap()
        .revision = "r2".into();
    ctx.store.save_config().unwrap();
    let error = approve_plan(&ctx, &id, &review.id).unwrap_err();
    assert!(
        format!("{error:#}").contains("stale plan verdict"),
        "{error:#}"
    );
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
fn same_planner_override_preview_matches_effective_family_policy() {
    for required in [None, Some(false), Some(true)] {
        // Given a task override pointing back to its planner under each policy.
        let (_tmp, mut ctx, id) = preview_fixture();
        ctx.store.config.review.require_cross_family = required;
        ctx.store.save_config().unwrap();
        crate::reviewer_set(&ctx, &id, Some("gpt-sol")).unwrap();
        // When previewing and actually starting the selected reviewer.
        let preview = crate::route(&ctx, &id, &ProfileChoice::default()).unwrap();
        assert_eq!(preview.require_cross_family, required == Some(true));
        let mapping = preview.review.plan_reviewer.unwrap();
        assert_eq!(mapping.profile.as_deref(), Some("gpt-sol"));
        assert_eq!(mapping.model.as_deref(), Some("fixture-planner"));
        assert!(!mapping.warnings.is_empty());
        let review = crate::plan_review(&ctx, &id, None);
        // Then only opt-in rejects it; default/off still starts a separate review.
        if required == Some(true) {
            assert!(review.is_err());
            assert!(ctx.store.get(&id).unwrap().plans[0].reviews.is_empty());
        } else {
            let review = review.unwrap();
            assert_eq!(review.reviewer, "gpt-sol");
            assert!(review.herdr.is_some());
        }
    }
}

#[test]
fn manual_plan_without_reviewer_previews_owner_instead_of_an_agent() {
    for design in [true, false] {
        // Given a manually submitted plan with no planner run.
        let (tmp, mut ctx, _) = preview_fixture();
        ctx.store.config.review.reviewer = Some("none".into());
        ctx.store.save_config().unwrap();
        let task = ctx
            .store
            .create("Manual plan", Some("app"), Status::Backlog, "")
            .unwrap();
        ctx.store
            .update(&task.id, |task| {
                task.profile = Some("gpt-sol".into());
                task.design = design;
                Ok(())
            })
            .unwrap();
        let path = tmp.path().join("manual-plan.md");
        std::fs::write(&path, PLAN).unwrap();
        let submitted = crate::plan_submit(&ctx, &task.id, &path).unwrap();
        assert!(submitted.run.is_none());
        let saved = ctx.store.get(&task.id).unwrap();
        assert_eq!(
            crate::plan::expected_reviewer(&ctx.store.config, &saved).unwrap(),
            "owner"
        );
        // When explaining the submitted plan and attempting actual plan review.
        let review = crate::route(&ctx, &task.id, &ProfileChoice::default())
            .unwrap()
            .review;
        let launch = crate::plan_review(&ctx, &task.id, None);
        // Then no planner or automatic reviewer is invented, and nothing launches.
        assert!(review.planner.is_none());
        let mapping = review.plan_reviewer.unwrap();
        assert_eq!(mapping.selection, "owner");
        assert_eq!(mapping.source, "owner");
        assert_eq!(mapping.profile, None);
        assert_eq!(mapping.model, None);
        assert_eq!(mapping.vendor, None);
        assert!(mapping.warnings.is_empty());
        assert!(launch.is_err());
        assert!(ctx.store.get(&task.id).unwrap().plans[0].reviews.is_empty());
    }
}

#[test]
fn submitted_plan_preview_uses_stored_planner_not_next_implementer() {
    // Given a submitted openai plan whose next implementer and current planner preset changed.
    let (_tmp, mut ctx, id) = preview_fixture();
    ctx.store.config.review.reviewer = Some("cross-vendor".into());
    let planner = ctx.store.config.profiles.get_mut("gpt-sol").unwrap();
    planner.model = Some("fixture-reviewer".into());
    planner.vendor = Some("anthropic".into());
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
    let agent = ctx.store.config.sync.omo_agent_dir.as_ref().unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"enabledModels":["fixture-reviewer"]}"#,
    )
    .unwrap();
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |task| {
            task.profile = Some("gpt-review".into());
            Ok(())
        })
        .unwrap();
    // When previewing and starting the submitted plan's reviewer.
    let preview = crate::route(&ctx, &id, &ProfileChoice::default()).unwrap();
    let review = crate::plan_review(&ctx, &id, None).unwrap();
    // Then the immutable planner identity, not either mutable preset, drives review.
    assert_eq!(preview.implementer.profile, "gpt-review");
    let planner = preview.review.planner.unwrap();
    assert_eq!(planner.profile, "gpt-sol");
    assert_eq!(planner.model.as_deref(), Some("fixture-planner"));
    assert_eq!(planner.vendor.as_deref(), Some("openai"));
    assert_eq!(
        serde_json::to_value(planner.source).unwrap(),
        serde_json::json!({"run":"D1"})
    );
    let mapping = preview.review.plan_reviewer.unwrap();
    assert_eq!(mapping.profile.as_deref(), Some("gpt-review"));
    assert_eq!(mapping.model.as_deref(), Some("fixture-reviewer"));
    assert_eq!(mapping.vendor.as_deref(), Some("anthropic"));
    assert!(mapping.warnings.is_empty());
    assert_eq!(review.reviewer, "gpt-review");
    assert!(review.herdr.is_some());
}

#[test]
fn explicit_replanning_preview_uses_future_route_instead_of_stored_planner() {
    // Given a submitted plan and an explicitly requested replacement with a new planner.
    let (_tmp, mut ctx, id) = preview_fixture();
    let mut future = ctx.store.config.profiles["gpt-sol"].clone();
    future.model = Some("future-planner-model".into());
    future.vendor = Some("anthropic".into());
    ctx.store
        .config
        .profiles
        .insert("future-planner".into(), future);
    let agent = ctx.store.config.sync.omo_agent_dir.as_ref().unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"enabledModels":["future-planner-model"]}"#,
    )
    .unwrap();
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |task| {
            task.profile = Some("future-planner".into());
            Ok(())
        })
        .unwrap();
    // When explaining explicit dispatch --plan.
    let preview = crate::route_with_plan(&ctx, &id, &ProfileChoice::default(), true).unwrap();
    // Then the replacement planner, rather than the prior plan's snapshot, is shown.
    let planner = preview.review.planner.unwrap();
    assert_eq!(planner.profile, "future-planner");
    assert_eq!(planner.model.as_deref(), Some("future-planner-model"));
    assert_eq!(planner.vendor.as_deref(), Some("anthropic"));
    assert_eq!(
        serde_json::to_value(planner.source).unwrap(),
        serde_json::json!({"source":"task"})
    );
    assert_eq!(
        preview.review.plan_reviewer.unwrap().profile.as_deref(),
        Some("gpt-review")
    );
}
