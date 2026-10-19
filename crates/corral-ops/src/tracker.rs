//! Trackers aggregate child completion; they never launch agent work.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{ApprovedScope, PlanDeliveryTarget, Status, Store, Task};
use serde_json::json;

use crate::{Change, Ctx};

pub(crate) fn validate_target(target: &PlanDeliveryTarget) -> Result<()> {
    match target {
        PlanDeliveryTarget::Main => Ok(()),
        PlanDeliveryTarget::Integration { branch } => {
            ensure!(branch != "main", "integration target must differ from main");
            corral_git::validate_branch_name(branch).context("invalid Plan integration branch")?;
            Ok(())
        }
    }
}

pub(crate) fn validate_scope(
    store: &Store,
    scope: &ApprovedScope,
    project: Option<&str>,
) -> Result<()> {
    ensure!(!scope.goal.trim().is_empty(), "Plan goal must not be blank");
    ensure!(
        !scope.acceptance.is_empty(),
        "Plan acceptance must not be empty"
    );
    validate_target(&scope.delivery_target)?;
    let mut criteria = std::collections::BTreeSet::new();
    for acceptance in &scope.acceptance {
        ensure!(
            !acceptance.scenario.trim().is_empty() && !acceptance.expected.trim().is_empty(),
            "blank Plan acceptance"
        );
        if let Some(id) = &acceptance.criterion {
            ensure!(
                !id.trim().is_empty() && criteria.insert(id),
                "blank or duplicate Plan criterion"
            );
        }
    }
    if let Some(link) = &scope.approved_design {
        let (id, revision) = link
            .split_once('@')
            .context("approved design must be <task>@<revision>")?;
        let task = store.get(id)?;
        let plan = task
            .plans
            .last()
            .filter(|plan| plan.id == revision && plan.approval.is_some())
            .context("stale approved design revision; link the current accepted plan")?;
        corral_core::plan_policy::validate_approval(store, &store.config, &task, plan)?;
        ensure!(
            task.project.as_deref() == project
                && scope.goal == plan.document.goal
                && scope.acceptance == plan.document.acceptance
                && scope.delivery_target
                    == plan.document.delivery_target.clone().unwrap_or_default(),
            "approved Plan scope differs from the linked design revision"
        );
    }
    Ok(())
}

pub(crate) fn check_action(task: &Task, action: &str) -> Result<()> {
    if task.tracker {
        bail!(
            "{} is a tracker; cannot {action}; dispatch its child tasks instead",
            task.id
        );
    }
    Ok(())
}

/// Read both the tracker and its children under the board writer lock. Returning
/// false from try_update preserves timestamps and concurrent owner edits.
pub(crate) fn reconcile_one(ctx: &Ctx, id: &str) -> Result<Option<Change>> {
    let mut from = None;
    let saved = ctx.store.try_update(id, |task| {
        if !task.tracker
            || matches!(task.status, Status::Done | Status::Dropped)
            || task.children.is_empty()
        {
            return Ok(false);
        }
        let children = ctx.store.list()?;
        if !task.children.iter().all(|id| {
            children
                .iter()
                .any(|child| child.id == *id && child.status == Status::Done)
        }) {
            return Ok(false);
        }
        from = Some(task.status);
        task.status = Status::Done;
        task.queued = false;
        task.dispatch_plan = None;
        Ok(true)
    })?;
    let Some(task) = saved else {
        return Ok(None);
    };
    ctx.store.log_event(
        Some(&task.id),
        "tracker_completed",
        json!({"from": from, "to": "done", "children": task.children}),
    )?;
    Ok(from.map(|from| Change {
        task: task.id,
        from,
        to: Status::Done,
        note: "all tracker children are Done".into(),
        stage: None,
    }))
}

pub(crate) fn reconcile(ctx: &Ctx) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if task.tracker
            && let Some(change) = reconcile_one(ctx, &task.id)?
        {
            changes.push(change);
        }
    }
    Ok(changes)
}

#[cfg(test)]
#[path = "tracker_tests.rs"]
mod tests;
