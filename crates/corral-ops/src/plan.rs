//! Plan snapshots and the optional pre-dispatch approval boundary.
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::{
    ArtifactKind, Config, PlanDocument, PlanRecord, ProfileChoice, Reviewer, Task, Verdict,
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
    let mut previous = None;
    let saved = ctx.store.update(id, |task| {
        check_criteria(task, &document)?;
        previous = task.plans.last().cloned();
        task.plans.push(PlanRecord {
            id: format!("P{}", task.plans.len() + 1),
            artifact,
            spec_revision: plan_spec_revision(task)?,
            document,
            reviews: Vec::new(),
        });
        ctx.store.log_event(
            Some(&task.id),
            "plan_submitted",
            json!({"target":"plan", "plan":task.plans.last()}),
        )?;
        Ok(())
    })?;
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
    match config
        .review_policy(task.project.as_deref(), task.review)
        .reviewer
    {
        Reviewer::None => Ok("owner".into()),
        Reviewer::Pm => Ok("pm".into()),
        Reviewer::Profile(profile) => Ok(profile),
        Reviewer::CrossVendor => {
            let route = config.route_task(task, &ProfileChoice::default(), |_| None)?;
            let resolved = config.resolve_profile(
                &ProfileChoice {
                    profile: Some(route.profile),
                    model: route.model,
                    effort: route.effort,
                },
                None,
                None,
            )?;
            config.cross_vendor_reviewer(
                resolved
                    .vendor
                    .as_deref()
                    .context("plan review requires a known implementer vendor")?,
            )
        }
    }
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

/// Fail before dispatch can create a worktree, pane, queue entry or run.
pub fn approved_plan(ctx: &Ctx, task: &Task) -> Result<Option<PlanRecord>> {
    let config = corral_core::Store::open(ctx.store.root())?.config;
    if !config.plan_required(task.project.as_deref(), task.plan_required) {
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
    if review.policy_digest != policy_digest(&config, task)?
        || review.reviewer != expected_reviewer(&config, task)?
    {
        bail!("stale plan approval: reviewer policy changed; review the plan again");
    }
    if owner_decisions(task, plan) {
        bail!("plan escalated: unresolved owner decision; resolve it and submit for review");
    }
    Ok(Some(plan.clone()))
}
