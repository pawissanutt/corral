//! Plan snapshots and the optional pre-dispatch approval boundary.
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::{ArtifactKind, Config, PlanDocument, PlanRecord, PlanRunState, Task, Verdict};
use serde_json::json;

use crate::{Ctx, add_artifact, verification};

pub(crate) mod member_add;
pub(crate) mod member_gate;
pub(crate) mod member_move;
pub(crate) mod scope_gate;

/// Revision of the task contract, including T-052 criterion IDs and dependencies.
pub fn plan_spec_revision(task: &Task) -> Result<String> {
    corral_core::plan_contract_revision(task)
}

pub(crate) fn read_plan(ctx: &Ctx, path: &Path) -> Result<Vec<u8>> {
    corral_core::plan_policy::read_plan(&ctx.store, path)
}

pub(crate) fn check_criteria(task: &Task, document: &PlanDocument) -> Result<()> {
    if let Some(manifest) = &task.completion {
        if manifest.spec_digest != verification::spec_digest(task)? {
            bail!("plan criteria are stale; register criteria for the current task spec first");
        }
        for scenario in &document.acceptance {
            let id = scenario
                .criterion
                .as_deref()
                .context("plan acceptance must reference registered criterion IDs")?;
            if !manifest.criteria.iter().any(|criterion| criterion.id == id) {
                bail!("plan references unknown criterion {id}");
            }
        }
        for criterion in manifest.criteria.iter().filter(|c| c.required) {
            if !document
                .acceptance
                .iter()
                .any(|a| a.criterion.as_deref() == Some(&criterion.id))
            {
                bail!("plan omits required criterion {}", criterion.id);
            }
        }
    }
    Ok(())
}

pub fn plan_submit(ctx: &Ctx, id: &str, path: &Path) -> Result<PlanRecord> {
    plan_submit_checked(ctx, id, path, &|_| Ok(()))
}

/// `plan_submit` whose final transaction first runs `check`: a native planner's binding.
pub(crate) fn plan_submit_checked(
    ctx: &Ctx,
    id: &str,
    path: &Path,
    check: &dyn Fn(&Task) -> Result<()>,
) -> Result<PlanRecord> {
    check(&ctx.store.get(id)?)?;
    crate::tracker::check_action(&ctx.store.get(id)?, "submit tracker to the plan pipeline")?;
    let bytes = read_plan(ctx, path)?;
    PlanDocument::parse(std::str::from_utf8(&bytes).context("plan must be UTF-8")?)?;
    let mut artifact = add_artifact(
        ctx,
        id,
        path,
        Some(ArtifactKind::Plan),
        Some("Implementation plan"),
    )?;
    // Artifact bytes deduplicate, but each submission binds its own source path.
    artifact.source = path
        .canonicalize()
        .with_context(|| format!("resolving submitted plan {}", path.display()))?
        .to_string_lossy()
        .into_owned();
    // Parse the exact stored snapshot, not a second read of a mutable source.
    let snapshot = read_plan(ctx, &ctx.store.dir().join(&artifact.path))?;
    let document =
        PlanDocument::parse(std::str::from_utf8(&snapshot).context("plan must be UTF-8")?)?;
    crate::plan_decompose::validate(&document)?;
    scope_gate::check_declared_tiers(
        &ctx.store.reopen()?.config,
        ctx.store.get(id)?.project.as_deref(),
        &document,
    )?;
    let mut previous = None;
    let saved = ctx.store.update(id, |task| {
        check(task)?;
        crate::tracker::check_action(task, "submit tracker to the plan pipeline")?;
        check_criteria(task, &document)?;
        let revision = plan_spec_revision(task)?;
        let plan_id = format!("P{}", task.plans.len() + 1);
        let run = match task.plan_runs.last_mut() {
            Some(run) if matches!(run.state, PlanRunState::Planning | PlanRunState::Changes) => {
                if run.spec_revision != revision {
                    bail!("stale planner run: task spec or criteria changed; start planning again");
                }
                run.submitted = Some(plan_id.clone());
                run.state = PlanRunState::Submitted;
                run.ended = Some(jiff::Timestamp::now());
                task.status = corral_core::Status::Review;
                Some(run.id.clone())
            }
            Some(_) => bail!("planner run is not accepting submissions"),
            None => None,
        };
        previous = task.plans.last().cloned();
        if let Some(plan) = task.plans.last_mut() {
            crate::plan_review::terminalize_pending(plan, "Superseded by a new plan submission.");
        }
        task.plans.push(PlanRecord {
            id: plan_id,
            artifact,
            spec_revision: revision,
            document,
            reviews: Vec::new(),
            run,
            approval: None,
        });
        ctx.store.log_event(
            Some(&task.id),
            "plan_submitted",
            json!({"target":"plan", "plan":task.plans.last()}),
        )?;
        Ok(())
    })?;
    crate::plan_dispatch::close_planner(ctx, &saved)?;
    if let Some(previous) = previous {
        crate::plan_review::close_plan_review(ctx, &previous)?;
    }
    saved.plans.last().cloned().context("plan was not saved")
}

/// Explicit task override; an absent value inherits project/workspace policy.
pub fn plan_policy_set(ctx: &Ctx, id: &str, required: Option<bool>) -> Result<Task> {
    let task = ctx.store.update(id, |task| {
        task.plan_required = required;
        if required == Some(true) {
            task.queued = false;
        }
        Ok(())
    })?;
    ctx.store
        .log_event(Some(&task.id), "plan_policy", json!({"required":required}))?;
    Ok(task)
}

pub(crate) fn config(ctx: &Ctx) -> Result<Config> {
    let store = ctx.store.reopen()?;
    store.config.validate_review()?;
    Ok(store.config)
}

pub(crate) use corral_core::plan_policy::{
    PlannerProfile, decisions_resolved, expected_reviewer, expected_reviewer_for,
    independent_reviewer, independent_reviewer_for, owner_decisions, planner, policy_digest,
};

pub(crate) fn fresh(ctx: &Ctx, task: &Task, plan: &PlanRecord) -> Result<()> {
    corral_core::plan_policy::fresh(&ctx.store, task, plan)
}

/// Only a final owner acceptance can resolve a manifest's design questions.
pub(crate) fn accepted_owner_notes(ctx: &Ctx, task: &Task) -> Result<Option<String>> {
    let Some((parent, revision)) = task.plan.as_deref().and_then(|link| link.split_once('@'))
    else {
        return Ok(None);
    };
    let parent = ctx.store.get(parent)?;
    let Some(plan) = parent
        .plans
        .iter()
        .find(|p| p.id == revision && p.approval.as_ref().is_some_and(|a| a.by == "owner"))
    else {
        return Ok(None);
    };
    Ok(plan
        .reviews
        .last()
        .filter(|r| r.reviewer == "owner" && r.verdict == Some(Verdict::Approve))
        .and_then(|r| r.notes.clone()))
}

/// Fail before dispatch can create a worktree, pane, queue entry or run.
pub fn approved_plan(ctx: &Ctx, task: &Task) -> Result<Option<PlanRecord>> {
    crate::tracker::check_action(task, "submit tracker to the plan pipeline")?;
    let config = ctx.store.reopen()?.config;
    if !task.design && !config.plan_required(task.project.as_deref(), task.plan_required) {
        return Ok(None);
    }
    config.validate_review()?;
    let plan = task.plans.last().context(
        "plan required: submit with `corral plan submit <id> <path>`, then review and approve it before dispatch",
    )?;
    corral_core::plan_policy::validate_approval(&ctx.store, &config, task, plan)?;
    Ok(Some(plan.clone()))
}

/// A Done design task whose accepted plan already published its children. Its
/// status is the dependency barrier for those children and must not be reopened.
pub(crate) fn published_design(task: &corral_core::Task) -> bool {
    task.status == corral_core::Status::Done
        && !task.children.is_empty()
        && task.plan.as_deref()
            == Some(&format!(
                "{}@{}",
                task.id,
                task.plans.last().map_or("", |p| &p.id)
            ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::{AgentCmd, PlanRun, Profile, Status, Store};

    fn reviewer_fixture() -> (tempfile::TempDir, Config, Task) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let mut task = store.create("Plan", None, Status::Ready, "Spec").unwrap();
        task.plan_runs.push(PlanRun {
            agent_host: None,
            execution: None,
            route: None,
            delivery: corral_core::inbox::DeliveryMode::Legacy,
            inbox: Default::default(),
            id: "PL1".into(),
            profile: "gpt-sol".into(),
            harness: "omo".into(),
            model: Some("fixture-planner".into()),
            vendor: Some("openai".into()),
            effort: None,
            spec_revision: plan_spec_revision(&task).unwrap(),
            started: jiff::Timestamp::now(),
            ended: None,
            herdr: None,
            output_dir: "output".into(),
            submitted: Some("P1".into()),
            rounds: 0,
            state: PlanRunState::Submitted,
            pending_reply: None,
        });
        let ctx = crate::resources::FixedResourceProbe::ctx(store);
        let path = tmp.path().join("plan.md");
        std::fs::write(&path, "---\ngoal: Plan\nnon_goals: []\ndecisions: []\naffected_paths: [a.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: check, expected: passes}]\nverification: [check]\nowner_decisions: []\n---\n").unwrap();
        let mut plan = plan_submit(&ctx, &task.id, &path).unwrap();
        plan.run = Some("PL1".into());
        task.plans.push(plan);
        let mut config = Config::default_for(tmp.path());
        config.agents.clear();
        config.profiles.clear();
        config.agents.insert(
            "omo".into(),
            AgentCmd {
                command: "true {model_args}".into(),
            },
        );
        for (name, model, vendor) in [
            ("gpt-sol", "fixture-planner", "openai"),
            ("same-model", "openai/fixture-planner", "openai"),
            ("reviewer", "fixture-reviewer", "openai"),
            ("other-vendor", "fixture-other", "anthropic"),
        ] {
            config.profiles.insert(
                name.into(),
                Profile {
                    harness: "omo".into(),
                    model: Some(model.into()),
                    vendor: Some(vendor.into()),
                    effort: None,
                    escalate_to: None,
                    args: vec![],
                    metadata: None,
                },
            );
        }
        (tmp, config, task)
    }

    #[test]
    fn configured_same_planner_pairing_is_allowed_by_default_and_explicit_off() {
        // Given the exact same-profile/model pairing that previously blocked plan reviews.
        for setting in [None, Some(false)] {
            let (_tmp, mut config, task) = reviewer_fixture();
            config.review.require_cross_family = setting;
            config.review.reviewer = Some("gpt-sol".into());
            // When selecting the configured reviewer.
            let reviewer = expected_reviewer(&config, &task).unwrap();
            // Then a fresh review session may use that profile, including a model alias.
            assert_eq!(reviewer, "gpt-sol");
            assert!(independent_reviewer(&config, &task, "same-model").is_ok());
        }
    }

    #[test]
    fn opted_in_review_uses_actual_planner_family_without_profile_specific_pairing() {
        // Given a planner and explicit cross-family enforcement.
        let (_tmp, mut config, task) = reviewer_fixture();
        config.review.require_cross_family = Some(true);
        // When choosing from configured profiles.
        let reviewer = expected_reviewer(&config, &task).unwrap();
        // Then the other vendor is accepted even for gpt-sol; same-family aliases are not.
        assert_eq!(reviewer, "other-vendor");
        assert!(independent_reviewer(&config, &task, "gpt-sol").is_err());
        assert!(independent_reviewer(&config, &task, "same-model").is_err());
        assert!(independent_reviewer(&config, &task, "reviewer").is_err());
        config.profiles.remove("other-vendor");
        assert!(expected_reviewer(&config, &task).is_err());
    }

    #[test]
    fn cross_vendor_policy_uses_planner_vendor_not_implementation_route() {
        let (_tmp, mut config, task) = reviewer_fixture();
        config.review.reviewer = Some("cross-vendor".into());
        config
            .routing
            .reviewer_by_vendor
            .insert("anthropic".into(), "other-vendor".into());
        assert_eq!(expected_reviewer(&config, &task).unwrap(), "other-vendor");
    }
}
