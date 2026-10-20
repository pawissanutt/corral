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
    /// Proposed dispatch base, not an assertion that the target ref exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_initial_base: Option<String>,
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
                proposed_initial_base: None,
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
            if let Some(binding) = &run.plan_target
                && binding.plan != plan.id
            {
                result.reason = Some(format!("{id}: accepted into another Plan; explicit target reconciliation required"));
                return Ok(result);
            }
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
            if run.plan_target.is_some()
                && !crate::plan_target::has_receipt(store, plan, &dependency, run, sha)?
            {
                result.reason = Some(format!("{id}: no published owned-target integration receipt"));
                return Ok(result);
            }
            let repo = store.project_path(
                plan.project
                    .as_deref()
                    .context("execution Plan has no project")?,
            )?;
            // Readiness may qualify a new target against the exact base its writer
            // will use. Owned initialized refs still require their actual Git head.
            result.proposed_initial_base = crate::plan_target::proposed_initial_base(store, plan)?;
            let head = match &result.proposed_initial_base {
                Some(base) => base.clone(),
                None => match corral_git::rev_sha(&repo, &format!("refs/heads/{target}")) {
                    Ok(head) => {
                        result.target_head = Some(head.clone());
                        head
                    }
                    Err(_) => {
                        result.reason = Some(format!("Plan target {target} is missing; initialize or reconcile it before dispatch"));
                        return Ok(result);
                    }
                },
            };
            result.satisfied = corral_git::is_ancestor(&repo, sha, &head)
                .with_context(|| format!("qualifying prerequisite {id} in {target}"))?;
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
    crate::plan_target::check_dispatch_target(store, task)?;
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
    let target = match &plan
        .approved_scope
        .as_ref()
        .context("Plan has no scope")?
        .delivery_target
    {
        PlanDeliveryTarget::Main => "main",
        PlanDeliveryTarget::Integration { branch } => branch,
    };
    if base != target {
        return Ok(Some(format!(
            "execution Plan requires {target} as its dispatch base"
        )));
    }
    let repo = store.project_path(plan.project.as_deref().context("Plan has no project")?)?;
    for prerequisite in prerequisites {
        if !prerequisite.satisfied {
            return Ok(prerequisite.reason.clone());
        }
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
    if let Some(plan) = store.execution_plan(task)? {
        if let Some(reason) =
            base_reason(store, &plan, &prerequisites(store, &plan, task)?, base, sha)?
        {
            bail!("{reason}");
        }
    } else {
        // Legacy metadata-only dependencies retain their status contract. A dependency
        // with a coding run must prove that its accepted commit is in this exact base,
        // even when the consumer is not itself a Plan member.
        for id in &task.depends_on {
            let dependency = store.get(id)?;
            let Some(run) = dependency.current_run() else {
                continue;
            };
            let receipt = store
                .events_shared()?
                .iter()
                .rev()
                .find(|event| {
                    event.kind == "approved"
                        && event.task.as_deref() == Some(id.as_str())
                        && event.detail["run"].as_str() == Some(run.id.as_str())
                })
                .cloned()
                .with_context(|| format!("{id}: no current integration receipt"))?;
            anyhow::ensure!(
                dependency.status == Status::Done
                    && run.outcome == Some(corral_core::RunOutcome::Done)
                    && matches!(
                        receipt.detail["strategy"].as_str(),
                        Some("fast_forward" | "rebase" | "keep_branch")
                    ),
                "{id}: prerequisite was not accepted into a target"
            );
            anyhow::ensure!(
                dependency.project == task.project,
                "{id}: prerequisite belongs to another repository"
            );
            let repo =
                store.project_path(task.project.as_deref().context("task has no project")?)?;
            let commit = receipt.detail["sha"]
                .as_str()
                .context("acceptance receipt has no commit SHA")?;
            anyhow::ensure!(
                corral_git::is_ancestor(&repo, commit, sha)?,
                "prepared dispatch base {base} does not include prerequisite {id}"
            );
        }
    }
    Ok(())
}
