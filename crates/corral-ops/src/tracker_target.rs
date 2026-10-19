//! Target qualification uses existing acceptance events and actual Git ancestry.
use anyhow::{Context, Result, bail};
use corral_core::{PlanDeliveryTarget, Status, Store, Task};
use serde::Serialize;

/// Acceptance and inclusion are separate facts about a prerequisite.
#[derive(Debug, Serialize)]
pub struct PlanPrerequisite {
    pub task: String,
    pub status: Status,
    pub external: bool,
    pub target: String,
    pub target_head: Option<String>,
    pub receipt_sha: Option<String>,
    pub design_publication: Option<String>,
    pub accepted: bool,
    pub satisfied: bool,
    pub reason: Option<String>,
}

pub(super) fn prerequisites(
    store: &Store,
    plan: &Task,
    task: &Task,
) -> Result<Vec<PlanPrerequisite>> {
    let target = match plan
        .approved_scope
        .as_ref()
        .map(|scope| &scope.delivery_target)
    {
        Some(PlanDeliveryTarget::Integration { branch }) => branch.as_str(),
        Some(PlanDeliveryTarget::Main) | None => "main",
    };
    task.depends_on
        .iter()
        .map(|id| {
            let dependency = store
                .get(id)
                .with_context(|| format!("{}: missing dependency {id}", task.id))?;
            let mut result = PlanPrerequisite {
                task: dependency.id.clone(),
                status: dependency.status,
                external: !plan.children.contains(id),
                target: target.into(),
                target_head: None,
                receipt_sha: None,
                design_publication: None,
                accepted: dependency.status == Status::Done,
                satisfied: false,
                reason: None,
            };
            if !result.accepted {
                result.reason = Some(format!("waiting on {id} ({})", dependency.status.label()));
                return Ok(result);
            }
            // Legacy trackers have no execution target contract.
            if plan.approved_scope.is_none() {
                result.satisfied = true;
                return Ok(result);
            }
            // Decomposition's exact approved design is a non-code publication receipt.
            if task.plan.as_ref().is_some_and(|link| {
                plan.approved_scope
                    .as_ref()
                    .and_then(|scope| scope.approved_design.as_ref())
                    == Some(link)
                    && dependency.plans.last().is_some_and(|design| {
                        link == &format!("{}@{}", dependency.id, design.id)
                            && dependency.children.contains(&task.id)
                    })
            }) {
                let design = dependency.plans.last().context("design receipt missing")?;
                corral_core::plan_policy::validate_approval(
                    store,
                    &store.config,
                    &dependency,
                    design,
                )?;
                result.satisfied = true;
                result.design_publication = task.plan.clone();
                return Ok(result);
            }
            if dependency.project != plan.project {
                result.reason = Some(format!("{id}: prerequisite belongs to another repository"));
                return Ok(result);
            }
            let Some(run) = dependency
                .current_run()
                .filter(|run| run.outcome == Some(corral_core::RunOutcome::Done))
            else {
                result.reason = Some(format!("{id}: accepted but no completed run receipt"));
                return Ok(result);
            };
            let events = store.events_shared()?;
            let receipt = events.iter().rev().find(|event| {
                event.kind == "approved"
                    && event.task.as_deref() == Some(id.as_str())
                    && event.detail["run"].as_str() == Some(run.id.as_str())
            });
            let Some(receipt) = receipt else {
                result.reason = Some(format!("{id}: accepted but no current acceptance receipt"));
                return Ok(result);
            };
            if !matches!(
                receipt.detail["strategy"].as_str(),
                Some("fast_forward" | "rebase")
            ) {
                result.reason = Some(format!(
                    "{id}: accepted branch was not integrated into {target}"
                ));
                return Ok(result);
            }
            let sha = receipt.detail["sha"]
                .as_str()
                .context("acceptance receipt has no commit SHA")?;
            result.receipt_sha = Some(sha.into());
            let repo = store.project_path(
                plan.project
                    .as_deref()
                    .context("execution Plan has no project")?,
            )?;
            // Missing refs or objects are explicit failures, never inferred inclusion.
            let head = corral_git::rev_sha(&repo, &format!("refs/heads/{target}"))
                .with_context(|| format!("reading Plan target {target}"))?;
            result.satisfied = corral_git::is_ancestor(&repo, sha, &head)
                .with_context(|| format!("qualifying prerequisite {id} in {target}"))?;
            result.target_head = Some(head);
            if !result.satisfied {
                result.reason = Some(format!("{id}: accepted work is not included in {target}"));
            }
            Ok(result)
        })
        .collect()
}

pub(crate) fn target_blocker(store: &Store, task: &Task) -> Result<Option<String>> {
    let Some(plan) = store.execution_plan(task)? else {
        return Ok(None);
    };
    if matches!(
        plan.approved_scope
            .as_ref()
            .map(|scope| &scope.delivery_target),
        Some(PlanDeliveryTarget::Integration { .. })
    ) {
        return Ok(Some(
            "Plan integration-target execution is not available; target metadata does not authorize a main-based run".into(),
        ));
    }
    let prerequisites = prerequisites(store, &plan, task)?;
    let reason = prerequisites
        .iter()
        .find(|dependency| !dependency.satisfied)
        .and_then(|dependency| dependency.reason.clone());
    if reason.is_some() {
        return Ok(reason);
    }
    if task.launch.is_some()
        && let Some(run) = task.current_run()
    {
        return base_reason(store, &plan, &prerequisites, &run.base, &run.base_sha);
    }
    Ok(None)
}

fn base_reason(
    store: &Store,
    plan: &Task,
    prerequisites: &[PlanPrerequisite],
    base: &str,
    sha: &str,
) -> Result<Option<String>> {
    if base != "main" {
        return Ok(Some(
            "execution Plan requires main as its dispatch base".into(),
        ));
    }
    let repo = store.project_path(plan.project.as_deref().context("Plan has no project")?)?;
    for prerequisite in prerequisites {
        if let Some(commit) = &prerequisite.receipt_sha
            && !corral_git::is_ancestor(&repo, commit, sha)?
        {
            return Ok(Some(format!(
                "prepared dispatch base does not include prerequisite {}; requeue from the current target",
                prerequisite.task,
            )));
        }
    }
    Ok(None)
}

pub(crate) fn check_dispatch_base(store: &Store, task: &Task, base: &str, sha: &str) -> Result<()> {
    if let Some(plan) = store.execution_plan(task)?
        && let Some(reason) =
            base_reason(store, &plan, &prerequisites(store, &plan, task)?, base, sha)?
    {
        bail!("{reason}");
    }
    Ok(())
}
