use super::*;
use serde_json::json;
use std::collections::BTreeMap;

pub(super) fn reviewer(config: &Config, name: &str) -> Result<RecoveryReviewer> {
    ensure!(
        !matches!(name, "owner" | "pm"),
        "recovery requires a separate reviewer profile"
    );
    let resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(name.into()),
            ..Default::default()
        },
        None,
        None,
    )?;
    Ok(RecoveryReviewer {
        profile: resolved.profile,
        model: resolved
            .model
            .context("reviewer effective model is unknown")?,
        vendor: resolved.vendor.context("reviewer vendor is unknown")?,
    })
}

pub(super) fn expected_code(config: &Config, task: &Task) -> Result<String> {
    let choice = crate::review_routing::choice(config, task, None);
    let choice = if choice == Reviewer::Auto {
        Reviewer::Profile(crate::review_routing::auto_profile(config, task)?)
    } else {
        choice
    };
    let family = task
        .current_run()
        .and_then(|r| r.family(r.vendor.as_deref()));
    crate::review_routing::resolve(config, &choice, family.as_ref()).map(|(name, _)| name)
}

/// Raw effective inputs are hashed *before* resolution, so a selector failure remains evidence.
pub(super) fn capture(
    config: &Config,
    task: &Task,
    action: &RecoveryAction,
) -> Result<RecoveryPolicy> {
    let effective = config.task_review_policy(task);
    let src = config.review_policy_sources(task.project.as_deref(), task.review);
    let mut sources = BTreeMap::from([
        (
            "reviewer-family-policy".into(),
            src.require_cross_family.label(task.project.as_deref()),
        ),
        (
            "configured-reviewer-binding".into(),
            if task.reviewer.is_some() {
                "task reviewer override".into()
            } else {
                src.reviewer.label(task.project.as_deref())
            },
        ),
        (
            "independent-plan-review".into(),
            src.plan_accept.label(task.project.as_deref()),
        ),
        (
            "gate-outcome".into(),
            src.gate.label(task.project.as_deref()),
        ),
    ]);
    if effective.reviewer == Reviewer::CrossVendor {
        sources.insert(
            "reviewer-family-policy".into(),
            sources["configured-reviewer-binding"].clone(),
        );
    }
    let mut inputs = BTreeMap::from([
        ("effective_policy".into(), serde_json::to_value(&effective)?),
        ("sources".into(), json!(sources)),
        (
            "plan_required".into(),
            json!(config.plan_required(task.project.as_deref(), task.plan_required)),
        ),
        ("task_reviewer".into(), json!(task.reviewer)),
        (
            "review_profiles".into(),
            serde_json::to_value(&config.routing.review_profiles)?,
        ),
        (
            "reviewer_by_vendor".into(),
            json!(config.routing.reviewer_by_vendor),
        ),
    ]);
    // Explicit selection only depends on these profiles; selectors may depend on the catalog.
    let expected = if matches!(
        action,
        RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::CodeReview,
            ..
        } | RecoveryAction::AcceptCode { .. }
    ) {
        expected_code(config, task)
    } else {
        crate::plan::expected_reviewer(config, task)
    };
    let mut names = BTreeSet::new();
    if let RecoveryAction::UseReviewer { reviewer, .. } = action {
        names.insert(reviewer.profile.clone());
    }
    if let Ok(name) = &expected {
        names.insert(if name == "pm" {
            config.pm.agent.clone()
        } else {
            name.clone()
        });
    }
    if let Some(planner) = crate::plan::planner(task)? {
        names.insert(planner.profile.clone());
    }
    if let Some(run) = task.current_run() {
        names.insert(run.agent.clone());
    }
    if !matches!(effective.reviewer, Reviewer::Profile(_) | Reviewer::Pm) {
        names.extend(config.profile_names());
    }
    let mut definitions = BTreeMap::new();
    for name in names {
        let raw = config.profiles.get(&name);
        let resolved = config.resolve_profile(
            &ProfileChoice {
                profile: Some(name.clone()),
                ..Default::default()
            },
            None,
            None,
        );
        definitions.insert(name, json!({"definition":raw, "resolved": resolved.map(|r| json!([r.profile,r.harness,r.command,r.model,r.vendor,r.effort,r.model_args])).map_err(|e| format!("{e:#}"))}));
    }
    inputs.insert("profiles".into(), json!(definitions));
    let digest = hash(&inputs)?;
    let (expected_reviewer, resolution_failure) = match expected {
        Ok(name) if matches!(name.as_str(), "owner" | "pm") => (None, None),
        Ok(name) => match reviewer(config, &name) {
            Ok(reviewer) => (Some(reviewer), None),
            Err(e) => (
                None,
                Some(RecoveryResolutionFailure {
                    code: "reviewer-resolution".into(),
                    message: format!("{e:#}"),
                    evidence: vec![name],
                }),
            ),
        },
        Err(e) => (
            None,
            Some(RecoveryResolutionFailure {
                code: "reviewer-resolution".into(),
                message: format!("{e:#}"),
                evidence: vec![],
            }),
        ),
    };
    Ok(RecoveryPolicy {
        digest,
        inputs,
        sources,
        expected_reviewer,
        resolution_failure,
    })
}
