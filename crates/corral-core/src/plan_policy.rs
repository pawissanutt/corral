//! Shared plan freshness, review policy and durable owner-resolution validation.
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

use crate::{
    Config, PlanDocument, PlanRecord, PlanRun, ProfileChoice, Reviewer, Store, Task, Verdict,
};

pub fn read_plan(store: &Store, path: &Path) -> Result<Vec<u8>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("opening plan {}", path.display()))?;
    if !file.metadata().context("reading plan metadata")?.is_file() {
        bail!("plan {} must be a regular file", path.display());
    }
    let max = store.config.artifacts.max_bytes;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading plan {}", path.display()))?;
    if u64::try_from(bytes.len())? > max {
        bail!("plan exceeds artifacts.max_bytes ({max})");
    }
    Ok(bytes)
}

pub fn expected_reviewer(config: &Config, task: &Task) -> Result<String> {
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
pub struct PlannerProfile<'a> {
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
pub fn expected_reviewer_for(
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

pub fn planner(task: &Task) -> Result<Option<&PlanRun>> {
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

pub fn independent_reviewer(config: &Config, task: &Task, name: &str) -> Result<()> {
    independent_reviewer_for(config, task, planner(task)?.map(PlannerProfile::from), name)
}

pub fn independent_reviewer_for(
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
    policy.check_reviewer_family(planner.vendor, reviewer.vendor.as_deref())?;
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

pub fn policy_digest(config: &Config, task: &Task) -> Result<String> {
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
    Ok(digest(&serde_json::to_vec(&(
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

pub fn fresh(store: &Store, task: &Task, plan: &PlanRecord) -> Result<()> {
    if crate::plan_contract_revision(task)? != plan.spec_revision {
        bail!(
            "stale plan {}: task spec or criteria changed; submit and review a new plan",
            plan.id
        );
    }
    for path in [
        store.dir().join(&plan.artifact.path),
        Path::new(&plan.artifact.source).to_path_buf(),
    ] {
        let bytes = read_plan(store, &path).with_context(|| {
            format!(
                "stale plan {}: restore its source or submit a new plan",
                plan.id
            )
        })?;
        if digest(&bytes) != plan.artifact.sha256 {
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

pub fn owner_decisions(task: &Task, plan: &PlanRecord) -> bool {
    !plan.document.owner_decisions.is_empty()
        || task
            .completion
            .as_ref()
            .is_some_and(|m| !m.owner_decisions.is_empty())
        || task
            .current_run()
            .is_some_and(|r| r.obligations.pending_decision().is_some())
}

pub fn decisions_resolved(task: &Task, plan: &PlanRecord, notes: &str) -> bool {
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

/// Validate the existing single-task plan acceptance contract without mutating it.
pub fn validate_approval(
    store: &Store,
    config: &Config,
    task: &Task,
    plan: &PlanRecord,
) -> Result<()> {
    if let Some(record) = approval_submission(store, config, task, plan)? {
        ensure!(
            matches!(record.disposition, crate::SubmissionDisposition::Submitted),
            "automatic plan approval does not match its submitted acceptance action"
        );
    }
    if task.plan.as_deref() != Some(&format!("{}@{}", task.id, plan.id)) {
        bail!("plan acceptance pending decomposition; sync or retry owner acceptance");
    }
    Ok(())
}

/// Validate the approval identity separately from its receipt and publication.
fn approval_submission(
    store: &Store,
    config: &Config,
    task: &Task,
    plan: &PlanRecord,
) -> Result<Option<crate::AuthorizedSubmission>> {
    config.validate_review()?;
    fresh(store, task, plan)?;
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
    let digest = policy_digest(config, task)?;
    let approval = plan
        .approval
        .as_ref()
        .context("plan pending: waiting for explicit owner approval")?;
    if review.policy_digest != digest || approval.policy_digest != digest {
        bail!("stale plan approval: reviewer policy changed; review the plan again");
    }
    let expected = expected_reviewer(config, task)?;
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
    } else if approval.by != expected || review.reviewer != expected || owner_decisions(task, plan)
    {
        bail!("stale plan approval: automatic acceptance policy or reviewer changed");
    }
    if approval.by != "owner" {
        let record = store.authorized_submission(
            approval
                .authorization
                .as_deref()
                .context("automatic plan approval has no scoped authorization")?,
        )?;
        let action = &record.authorization.action;
        ensure!(
            matches!(
                record.authorization.origin,
                crate::OperatingOrigin::Automatic { .. }
            ) && action.kind == crate::OperatingActionKind::AcceptPlan
                && action.task == task.id
                && action.plan.as_deref() == Some(plan.id.as_str())
                && action.run == plan.run.as_deref().unwrap_or(plan.id.as_str())
                && action.attempt == format!("accept-{}", review.id),
            "automatic plan approval does not match its submitted acceptance action"
        );
        return Ok(Some(record));
    }
    Ok(None)
}

impl Store {
    /// Finish the receipt of an already durable local automatic plan approval.
    ///
    /// This transition has no IPC to replay. Validate the exact saved approval
    /// and current authority under the board/task locks before completing proof.
    /// `check` retains the executor's publication restrictions and must not write.
    pub fn reconcile_plan_acceptance(
        &self,
        id: &str,
        check: impl FnOnce(&Store, &Task) -> Result<()>,
    ) -> Result<()> {
        let id = Self::normalize_id(id)?;
        let _board = self.lock("dispatch")?;
        let _task = self.lock(&id)?;
        let mut fresh = self.clone();
        fresh.config = self.operating_config()?;
        let task = fresh.get(&id)?;
        let plan = task.plans.last().context("accepted plan missing")?;
        let mut record = approval_submission(&fresh, &fresh.config, &task, plan)?
            .context("local acceptance recovery requires an automatic approval")?;
        match record.disposition {
            crate::SubmissionDisposition::Prepared | crate::SubmissionDisposition::Submitting => {
                fresh.validate_operating_action(&fresh.config, &task, &record.authorization)?;
                check(&fresh, &task)?;
                record.disposition = crate::SubmissionDisposition::Submitted;
                fresh.save_authorized_submission(&record)
            }
            crate::SubmissionDisposition::Submitted => Ok(()),
            crate::SubmissionDisposition::AuthorizationRefused { .. }
            | crate::SubmissionDisposition::Uncertain { .. } => {
                bail!("local acceptance receipt is not a recoverable write boundary")
            }
        }
    }
}

/// Validate answers on a task's accepted plan or its published parent's exact revision.
pub fn validate_owner_resolution(store: &Store, config: &Config, task: &Task) -> Result<()> {
    if let Some(plan) = task.plans.last() {
        return validate_approval(store, config, task, plan);
    }
    let (parent_id, revision) = task
        .plan
        .as_deref()
        .and_then(|link| link.split_once('@'))
        .context("owner decisions have no accepted plan")?;
    let parent = store.get(parent_id)?;
    ensure!(
        parent.status == crate::Status::Done && parent.children.contains(&task.id),
        "owner resolution is not attached to a published child"
    );
    let plan = parent
        .plans
        .last()
        .filter(|plan| plan.id == revision)
        .context("owner resolution references a missing or replaced parent plan")?;
    validate_approval(store, config, &parent, plan)?;
    ensure!(
        plan.approval
            .as_ref()
            .is_some_and(|approval| approval.by == "owner"),
        "inherited owner decisions require owner acceptance"
    );
    ensure!(
        parent
            .current_run()
            .is_none_or(|run| run.obligations.pending_decision().is_none()),
        "parent has a pending owner decision"
    );
    let notes = plan
        .reviews
        .last()
        .and_then(|review| review.notes.as_deref())
        .unwrap_or("");
    ensure!(
        decisions_resolved(task, plan, notes),
        "inherited owner decisions require explicit resolutions"
    );
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
