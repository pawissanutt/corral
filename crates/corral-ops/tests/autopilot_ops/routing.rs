use super::*;
use corral_core::{Harness, Profile, ProfileChoice};
use corral_ops::ReviewMapping;

fn configured(reviewer: &str) -> Fx {
    let mut fx = Fx::new(|policy| {
        policy.gate = gate(&["true"]);
        policy.reviewer = Some("none".into());
        policy.accept = Some(Accept::Owner);
    });
    fx.ctx.store.config.harnesses.insert(
        "routing-test".into(),
        Harness {
            command: "true {model_args}".into(),
            flags: Some("omo".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    for (name, vendor) in [
        ("fake", "anthropic"),
        ("cross", "openai"),
        ("workspace", "openai"),
        ("project", "google"),
        ("override", "deepseek"),
        ("pm-profile", "openai"),
        ("same", "anthropic"),
    ] {
        fx.ctx.store.config.profiles.insert(
            name.into(),
            Profile {
                harness: "routing-test".into(),
                vendor: Some(vendor.into()),
                model: None,
                effort: None,
                escalate_to: None,
                args: vec![],
            },
        );
    }
    fx.ctx.store.config.routing.reviewer_by_vendor = [
        ("openai".into(), "cross".into()),
        ("anthropic".into(), "fake".into()),
    ]
    .into();
    fx.ctx.store.config.pm.agent = "pm-profile".into();
    fx.ctx.store.config.review.reviewer = Some(reviewer.into());
    fx.ctx.store.save_config().unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            // Fx dispatched with an explicit choice; pin the next-run preview to that choice too.
            task.profile = Some("fake".into());
            let run = task.current_run_mut().unwrap();
            run.vendor = Some("anthropic".into());
            run.harness = Some("routing-test".into());
            Ok(())
        })
        .unwrap();
    fx.change("widget.txt", "spins\n");
    fx
}

fn preview(fx: &Fx) -> ReviewMapping {
    let before = fx.task();
    let events = fx.ctx.store.events().unwrap().len();
    let explanation = corral_ops::route(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    assert_eq!(explanation.implementer.profile, "fake");
    assert_eq!(explanation.implementer.harness, "routing-test");
    assert_eq!(fx.task(), before, "preview must not change the task");
    assert_eq!(fx.ctx.store.events().unwrap().len(), events);
    explanation.review.reviewer
}

fn selected(fx: &Fx, expected: &str, source: &str, policy_source: &str) -> ReviewMapping {
    let mapping = preview(fx);
    assert_eq!(mapping.profile.as_deref(), Some(expected));
    assert_eq!(mapping.harness.as_deref(), Some("routing-test"));
    assert_eq!(mapping.model, None);
    assert_eq!(mapping.source, source);
    assert_eq!(mapping.policy_source, policy_source);
    mapping
}

fn assert_autopilot_selection(fx: &Fx, mapping: &ReviewMapping, expected_record: &str) {
    // Preview is deliberately captured before done freezes the candidate.
    fx.done();
    let task = fx.sync();
    assert_eq!(task.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let run = task.current_run().unwrap();
    assert_eq!(run.agent, "fake");
    assert_eq!(run.review_runs.len(), 1);
    let child = &run.review_runs[0];
    assert_eq!(Some(child.agent.as_str()), mapping.profile.as_deref());
    assert_eq!(child.harness, mapping.harness);
    assert_eq!(child.model, mapping.model);
    assert_eq!(child.vendor, mapping.vendor);
    let record = run.review.as_ref().unwrap();
    assert_eq!(record.reviewer, expected_record);
    assert_eq!(record.sha, git(&run.worktree, &["rev-parse", "HEAD"]));
    assert!(
        record.pane.is_some(),
        "the reviewer must actually be launched"
    );
    assert_eq!(child.review.as_ref().unwrap().reviewer, expected_record);
    assert_eq!(fx.events("review_started"), 1);
}

#[test]
fn cross_vendor_preview_matches_autopilot_child_and_record() {
    // Given an anthropic implementer and an independently configured openai reviewer.
    let fx = configured("cross-vendor");
    let mapping = selected(&fx, "cross", "cross_vendor_map", "workspace_review_config");
    assert_eq!(mapping.selection, "cross-vendor");
    assert_eq!(mapping.vendor.as_deref(), Some("openai"));
    assert!(mapping.warnings.is_empty(), "{mapping:?}");
    // When autopilot reviews the finished candidate, then it uses the previewed profile.
    assert_autopilot_selection(&fx, &mapping, "cross");
}

#[test]
fn workspace_profile_preview_matches_autopilot_child_and_record() {
    // Given an explicit workspace reviewer rather than the cross-vendor mapping.
    let fx = configured("workspace");
    let mapping = selected(
        &fx,
        "workspace",
        "workspace_review_config",
        "workspace_review_config",
    );
    assert_eq!(mapping.selection, "workspace");
    // When autopilot reviews, then it preserves that explicit profile.
    assert_autopilot_selection(&fx, &mapping, "workspace");
}

#[test]
fn project_profile_supersedes_workspace_in_preview_and_autopilot() {
    // Given distinct workspace and project reviewer profiles.
    let mut fx = configured("workspace");
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .reviewer = Some("project".into());
    fx.ctx.store.save_config().unwrap();
    let mapping = selected(
        &fx,
        "project",
        "project_review_config",
        "project_review_config",
    );
    assert_eq!(mapping.selection, "project");
    // When autopilot reviews, then the project profile wins.
    assert_autopilot_selection(&fx, &mapping, "project");
}

#[test]
fn pm_preview_matches_accounting_child_and_pm_review_record() {
    // Given a PM backed by a profile distinct from every other reviewer.
    let fx = configured("pm");
    std::fs::write(
        fx.ctx.store.state_dir().join("pm.json"),
        r#"{"pane_id":"w1:p1"}"#,
    )
    .unwrap();
    let mapping = selected(&fx, "pm-profile", "pm", "workspace_review_config");
    assert_eq!(mapping.selection, "pm");
    // When autopilot requests PM review, then its child records the PM profile, not "pm".
    assert_autopilot_selection(&fx, &mapping, "pm");
    assert_eq!(fx.run().review.unwrap().pane.as_deref(), Some("w1:p1"));
    assert!(!fx.herdr_log("prompts.log").is_empty());
}

#[test]
fn none_preview_starts_no_reviewer_after_autopilot_gate() {
    // Given an explicitly disabled reviewer.
    let fx = configured("none");
    let mapping = preview(&fx);
    assert_eq!(mapping.selection, "none");
    assert_eq!(mapping.source, "none");
    assert_eq!(mapping.profile, None);
    assert_eq!(mapping.harness, None);
    assert!(mapping.warnings.is_empty());
    // When autopilot processes the completed candidate.
    fx.done();
    fx.sync();
    // Then the gate passed without launching or recording a reviewer.
    let run = fx.run();
    assert_eq!(run.gate.unwrap().result, GateResult::Pass);
    assert!(run.review.is_none());
    assert!(run.review_runs.is_empty());
    assert_eq!(fx.events("review_started"), 0);
    assert!(fx.herdr_log("prompts.log").is_empty());
}

#[test]
fn task_override_supersedes_project_and_workspace_for_autopilot() {
    // Given three distinct profiles at the workspace, project and task layers.
    let mut fx = configured("workspace");
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .reviewer = Some("project".into());
    fx.ctx.store.save_config().unwrap();
    assert_eq!(preview(&fx).profile.as_deref(), Some("project"));
    // When setting the task override.
    let saved = corral_ops::reviewer_set(&fx.ctx, &fx.id, Some("override")).unwrap();
    assert_eq!(saved.reviewer.as_deref(), Some("override"));
    assert_eq!(
        Store::open(fx.ctx.store.root())
            .unwrap()
            .get(&fx.id)
            .unwrap()
            .reviewer,
        saved.reviewer
    );
    // Then both preview and actual autopilot use it instead of either fallback.
    let mapping = selected(&fx, "override", "task_override", "task_override");
    assert_eq!(mapping.selection, "override");
    assert_autopilot_selection(&fx, &mapping, "override");
}

#[test]
fn clearing_task_override_restores_project_for_preview_and_autopilot() {
    // Given a task override hiding a distinct project reviewer and workspace fallback.
    let mut fx = configured("workspace");
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .reviewer = Some("project".into());
    fx.ctx.store.save_config().unwrap();
    corral_ops::reviewer_set(&fx.ctx, &fx.id, Some("override")).unwrap();
    assert_eq!(preview(&fx).profile.as_deref(), Some("override"));
    // When clearing the task override.
    let saved = corral_ops::reviewer_set(&fx.ctx, &fx.id, None).unwrap();
    assert_eq!(saved.reviewer, None);
    // Then project routing resumes, not the workspace or stale task profile.
    let mapping = selected(
        &fx,
        "project",
        "project_review_config",
        "project_review_config",
    );
    assert_autopilot_selection(&fx, &mapping, "project");
}

#[test]
fn clearing_task_override_without_project_restores_workspace() {
    // Given a task override with only a distinct workspace fallback.
    let fx = configured("workspace");
    corral_ops::reviewer_set(&fx.ctx, &fx.id, Some("override")).unwrap();
    assert_eq!(preview(&fx).profile.as_deref(), Some("override"));
    // When clearing the override.
    corral_ops::reviewer_set(&fx.ctx, &fx.id, None).unwrap();
    // Then preview and autopilot return to the workspace reviewer.
    let mapping = selected(
        &fx,
        "workspace",
        "workspace_review_config",
        "workspace_review_config",
    );
    assert_autopilot_selection(&fx, &mapping, "workspace");
}

#[test]
fn explicit_same_vendor_profile_warns_but_still_matches_autopilot() {
    // Given an explicitly chosen reviewer with the implementer's vendor.
    let fx = configured("same");
    let mapping = selected(
        &fx,
        "same",
        "workspace_review_config",
        "workspace_review_config",
    );
    // When previewing, then the warning is visible without silently changing profiles.
    assert_eq!(mapping.vendor.as_deref(), Some("anthropic"));
    assert!(!mapping.warnings.is_empty(), "{mapping:?}");
    assert_autopilot_selection(&fx, &mapping, "same");
}

#[test]
fn unmapped_cross_vendor_preview_warns_without_launching_or_falling_back() {
    // Given no mapping for the vendor opposite the implementer.
    let mut fx = configured("cross-vendor");
    fx.ctx
        .store
        .config
        .routing
        .reviewer_by_vendor
        .remove("openai");
    fx.ctx.store.save_config().unwrap();
    // When previewing, then no unrelated reviewer is substituted and the task stays untouched.
    let mapping = preview(&fx);
    assert_eq!(mapping.selection, "cross-vendor");
    assert_eq!(mapping.source, "cross_vendor_map");
    assert_eq!(mapping.profile, None);
    assert!(!mapping.warnings.is_empty(), "{mapping:?}");
    assert!(fx.run().review_runs.is_empty());
    assert_eq!(fx.events("review_started"), 0);
}

#[test]
fn unresolved_task_frontmatter_warns_instead_of_using_configured_fallbacks() {
    // Given unresolved frontmatter, which bypasses the validated reviewer_set entry point.
    let mut fx = configured("workspace");
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .reviewer = Some("project".into());
    fx.ctx.store.save_config().unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.reviewer = Some("missing-review-profile".into());
            Ok(())
        })
        .unwrap();
    // When previewing, then the unresolved override remains visible with a warning, not a fallback.
    let mapping = preview(&fx);
    assert_eq!(mapping.selection, "missing-review-profile");
    assert_eq!(mapping.source, "task_override");
    assert_eq!(mapping.policy_source, "task_override");
    assert_eq!(mapping.harness, None);
    assert!(!mapping.warnings.is_empty(), "{mapping:?}");
    assert!(fx.run().review_runs.is_empty());
}
