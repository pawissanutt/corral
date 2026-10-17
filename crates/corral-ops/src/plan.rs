//! Plan snapshots and the optional pre-dispatch approval boundary.
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::{
    ArtifactKind, Config, PlanDocument, PlanRecord, PlanRun, PlanRunState, ProfileChoice, Reviewer,
    Task, Verdict,
};
use serde_json::json;

use crate::{Ctx, add_artifact, verification};

/// Revision of the task contract, including T-052 criterion IDs and dependencies.
pub fn plan_spec_revision(task: &Task) -> Result<String> {
    Ok(verification::digest(&serde_json::to_vec(&(
        verification::spec_digest(task)?,
        &task.completion,
        &task.depends_on,
    ))?))
}

pub(crate) fn read_plan(ctx: &Ctx, path: &Path) -> Result<Vec<u8>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("opening plan {}", path.display()))?;
    if !file.metadata().context("reading plan metadata")?.is_file() {
        bail!("plan {} must be a regular file", path.display());
    }
    let max = ctx.store.config.artifacts.max_bytes;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading plan {}", path.display()))?;
    if u64::try_from(bytes.len())? > max {
        bail!("plan exceeds artifacts.max_bytes ({max})");
    }
    Ok(bytes)
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
    let mut previous = None;
    let saved = ctx.store.update(id, |task| {
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
    let store = corral_core::Store::open(ctx.store.root())?;
    store.config.validate_review()?;
    Ok(store.config)
}

pub(crate) fn expected_reviewer(config: &Config, task: &Task) -> Result<String> {
    let planner = planner(task)?.map(PlannerProfile::from);
    let vendor =
        if planner.is_none() && config.task_review_policy(task).reviewer == Reviewer::CrossVendor {
            let route = config.route_task(task, &ProfileChoice::default(), |_| None)?;
            config
                .resolve_profile(
                    &ProfileChoice {
                        profile: Some(route.profile),
                        model: route.model,
                        effort: route.effort,
                    },
                    None,
                    None,
                )?
                .vendor
        } else {
            None
        };
    expected_reviewer_for(config, task, planner, vendor.as_deref())
}

#[derive(Clone, Copy)]
pub(crate) struct PlannerProfile<'a> {
    pub profile: &'a str,
    pub model: Option<&'a str>,
    pub vendor: Option<&'a str>,
}

impl<'a> From<&'a PlanRun> for PlannerProfile<'a> {
    fn from(run: &'a PlanRun) -> Self {
        Self {
            profile: &run.profile,
            model: run.model.as_deref(),
            vendor: run.vendor.as_deref(),
        }
    }
}

/// Also accepts a prospective planner so pre-dispatch preview enforces launch-time independence.
pub(crate) fn expected_reviewer_for(
    config: &Config,
    task: &Task,
    planner: Option<PlannerProfile<'_>>,
    implementer_vendor: Option<&str>,
) -> Result<String> {
    let policy = config.task_review_policy(task);
    match policy.reviewer.clone() {
        Reviewer::None if planner.is_none() => Ok("owner".into()),
        Reviewer::None => config
            .profile_names()
            .into_iter()
            .find(|name| independent_reviewer_for(config, task, planner, name).is_ok())
            .with_context(|| {
                format!(
                    "no configured plan reviewer satisfies effective require_cross_family = {}",
                    policy.cross_family_required()
                )
            }),
        Reviewer::Pm => Ok("pm".into()),
        Reviewer::Profile(profile) => Ok(profile),
        Reviewer::CrossVendor => {
            if let Some(planner) = planner {
                return config
                    .cross_vendor_reviewer(
                        planner
                            .vendor
                            .context("plan review requires a known planner vendor")?,
                    )
                    .and_then(|name| {
                        independent_reviewer_for(config, task, Some(planner), &name)?;
                        Ok(name)
                    });
            }
            config.cross_vendor_reviewer(
                implementer_vendor.context("plan review requires a known implementer vendor")?,
            )
        }
    }
}

pub(crate) fn planner(task: &Task) -> Result<Option<&PlanRun>> {
    match task.plans.last().and_then(|p| p.run.as_deref()) {
        Some(id) => task
            .plan_runs
            .iter()
            .find(|r| r.id == id)
            .map(Some)
            .context("plan references a missing planner run"),
        None => Ok(None),
    }
}

pub(crate) fn independent_reviewer(config: &Config, task: &Task, name: &str) -> Result<()> {
    independent_reviewer_for(config, task, planner(task)?.map(PlannerProfile::from), name)
}

pub(crate) fn independent_reviewer_for(
    config: &Config,
    task: &Task,
    planner: Option<PlannerProfile<'_>>,
    name: &str,
) -> Result<()> {
    if name == "owner" {
        return Ok(());
    }
    let policy = config.task_review_policy(task);
    let profile = if name == "pm" {
        config.pm.agent.as_str()
    } else {
        name
    };
    let reviewer = config.resolve_profile(
        &ProfileChoice {
            profile: Some(profile.into()),
            ..Default::default()
        },
        None,
        None,
    )?;
    if !policy.cross_family_required() {
        return Ok(());
    }
    let Some(planner) = planner else {
        let route = config.route_task(task, &ProfileChoice::default(), |_| None)?;
        let author = config.resolve_profile(
            &ProfileChoice {
                profile: Some(route.profile),
                model: route.model,
                effort: route.effort,
            },
            None,
            None,
        )?;
        return policy.check_reviewer_family(author.vendor.as_deref(), reviewer.vendor.as_deref());
    };
    policy.check_reviewer_family(planner.vendor.as_deref(), reviewer.vendor.as_deref())?;
    let planner_model = planner
        .model
        .context("planner effective model is unknown")?;
    let reviewer_model = reviewer
        .model
        .as_deref()
        .context("reviewer effective model is unknown")?;
    let model_name = |model: &str| {
        model
            .rsplit(['/', ':'])
            .next()
            .unwrap_or(model)
            .to_ascii_lowercase()
    };
    if reviewer.profile == planner.profile
        || model_name(reviewer_model) == model_name(planner_model)
    {
        bail!(
            "cross-family review is required: plan reviewer must use a different profile and effective model than the planner"
        );
    }
    Ok(())
}

pub(crate) fn policy_digest(config: &Config, task: &Task) -> Result<String> {
    let expected = expected_reviewer(config, task)?;
    let profile = match expected.as_str() {
        "owner" => None,
        name => Some(config.resolve_profile(
            &ProfileChoice {
                profile: Some(if name == "pm" {
                    config.pm.agent.clone()
                } else {
                    name.into()
                }),
                ..Default::default()
            },
            None,
            None,
        )?),
    };
    Ok(verification::digest(&serde_json::to_vec(&(
        config.plan_required(task.project.as_deref(), task.plan_required),
        task.design,
        config
            .review_policy(task.project.as_deref(), task.review)
            .plan_accept,
        config.task_review_policy(task).cross_family_required(),
        planner(task)?.map(|r| (&r.id, &r.profile, &r.model, &r.vendor)),
        expected,
        profile.map(|p| {
            (
                p.profile,
                p.harness,
                p.command,
                p.model,
                p.vendor,
                p.effort,
                p.model_args,
            )
        }),
    ))?))
}

pub(crate) fn fresh(ctx: &Ctx, task: &Task, plan: &PlanRecord) -> Result<()> {
    if plan_spec_revision(task)? != plan.spec_revision {
        bail!(
            "stale plan {}: task spec or criteria changed; submit and review a new plan",
            plan.id
        );
    }
    for path in [
        ctx.store.dir().join(&plan.artifact.path),
        Path::new(&plan.artifact.source).to_path_buf(),
    ] {
        let bytes = read_plan(ctx, &path).with_context(|| {
            format!(
                "stale plan {}: restore its source or submit a new plan",
                plan.id
            )
        })?;
        if verification::digest(&bytes) != plan.artifact.sha256 {
            bail!(
                "stale plan {}: plan bytes changed at {}; submit and review a new plan",
                plan.id,
                path.display()
            );
        }
        if PlanDocument::parse(std::str::from_utf8(&bytes).context("plan must be UTF-8")?)?
            != plan.document
        {
            bail!(
                "stale plan {}: stored metadata differs from approved bytes; submit again",
                plan.id
            );
        }
    }
    Ok(())
}

pub(crate) fn owner_decisions(task: &Task, plan: &PlanRecord) -> bool {
    !plan.document.owner_decisions.is_empty()
        || task
            .completion
            .as_ref()
            .is_some_and(|m| !m.owner_decisions.is_empty())
        || task
            .current_run()
            .is_some_and(|r| r.obligations.pending_decision().is_some())
}

pub(crate) fn decisions_resolved(task: &Task, plan: &PlanRecord, notes: &str) -> bool {
    let resolved = |question: &str| {
        notes.lines().any(|line| {
            line.strip_prefix("Resolved: ")
                .and_then(|s| s.split_once(" => "))
                .is_some_and(|(decision, resolution)| {
                    decision.trim() == question.trim() && !resolution.trim().is_empty()
                })
        })
    };
    plan.document
        .owner_decisions
        .iter()
        .chain(task.completion.iter().flat_map(|m| &m.owner_decisions))
        .all(|question| resolved(question))
        && task.current_run().is_none_or(|r| {
            r.obligations
                .decisions
                .iter()
                .filter(|d| d.pending())
                .all(|d| resolved(&d.request.question))
        })
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
    let config = corral_core::Store::open(ctx.store.root())?.config;
    if !task.design && !config.plan_required(task.project.as_deref(), task.plan_required) {
        return Ok(None);
    }
    config.validate_review()?;
    let plan = task.plans.last().context(
        "plan required: submit with `corral plan submit <id> <path>`, then review and approve it before dispatch",
    )?;
    fresh(ctx, task, plan)?;
    let review = plan.reviews.last().context(
        "plan pending: run `corral plan review <id> --reviewer <profile|pm>` and record a plan verdict",
    )?;
    match review.verdict {
        Some(Verdict::Approve) => {}
        Some(Verdict::Changes) => {
            bail!("plan rejected: address review notes, submit a new plan and review it")
        }
        Some(Verdict::Escalate) => {
            bail!("plan escalated: resolve owner decisions and submit for review")
        }
        None => bail!("plan pending: waiting for a plan verdict before explicit dispatch"),
    }
    let digest = policy_digest(&config, task)?;
    let approval = plan
        .approval
        .as_ref()
        .context("plan pending: waiting for explicit owner approval")?;
    if review.policy_digest != digest || approval.policy_digest != digest {
        bail!("stale plan approval: reviewer policy changed; review the plan again");
    }
    let expected = expected_reviewer(&config, task)?;
    if approval.by == "owner" {
        if review.reviewer != "owner"
            || (expected != "owner"
                && !plan.reviews.iter().rev().nth(1).is_some_and(|r| {
                    r.reviewer == expected
                        && r.verdict == Some(Verdict::Approve)
                        && r.policy_digest == digest
                }))
        {
            bail!("stale plan approval: independent review missing");
        }
        if !decisions_resolved(task, plan, review.notes.as_deref().unwrap_or("")) {
            bail!("plan escalated: owner decisions require explicit resolutions in notes");
        }
    } else if approval.by != expected
        || review.reviewer != expected
        || config
            .review_policy(task.project.as_deref(), task.review)
            .plan_accept
            != corral_core::Accept::Auto
        || owner_decisions(task, plan)
    {
        bail!("stale plan approval: automatic acceptance policy or reviewer changed");
    }
    if task.plan.as_deref() != Some(&format!("{}@{}", task.id, plan.id)) {
        bail!("plan acceptance pending decomposition; sync or retry owner acceptance");
    }
    Ok(Some(plan.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::{AgentCmd, Profile, Status, Store};

    fn reviewer_fixture() -> (tempfile::TempDir, Config, Task) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let mut task = store.create("Plan", None, Status::Ready, "Spec").unwrap();
        task.plan_runs.push(PlanRun {
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
