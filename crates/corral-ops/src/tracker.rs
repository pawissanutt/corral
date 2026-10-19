//! Trackers aggregate child completion; they never launch agent work.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{ApprovedScope, PlanAcceptance, PlanDeliveryTarget, Status, Store, Task};
use serde::Serialize;
use serde_json::json;

use crate::{Change, Ctx};

#[path = "tracker_target.rs"]
mod target;
pub use target::PlanPrerequisite;
pub(crate) use target::{check_dispatch_base, target_blocker};

/// Mutually exclusive work buckets; acceptance does not imply target inclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanWorkState {
    Accepted,
    Running,
    Review,
    Waiting,
    Missing,
}

/// Counts always retain every required member, including dropped work.
#[derive(Debug, Default, Serialize)]
pub struct PlanWorkCounts {
    pub accepted: usize,
    pub running: usize,
    pub review: usize,
    pub waiting: usize,
    pub missing: usize,
}

/// One normal task and its current readiness, not a separate scheduler entry.
#[derive(Debug, Serialize)]
pub struct PlanWork {
    pub task: String,
    pub title: String,
    pub status: Status,
    pub state: PlanWorkState,
    pub run: Option<String>,
    pub reason: Option<String>,
    pub prerequisites: Vec<PlanPrerequisite>,
}

/// Mapping is coverage only, never evidence that an aggregate scenario passed.
#[derive(Debug, Serialize)]
pub struct PlanCoverage {
    pub acceptance: PlanAcceptance,
    pub tasks: Vec<String>,
}

/// Shared, read-only tracker projection for CLI and board consumers.
#[derive(Debug, Serialize)]
pub struct PlanProgress {
    pub task: String,
    pub status: Status,
    pub approved_scope: Option<ApprovedScope>,
    pub scope_revision: u64,
    pub counts: PlanWorkCounts,
    pub work: Vec<PlanWork>,
    pub coverage: Vec<PlanCoverage>,
    pub unmapped_criteria: Vec<PlanAcceptance>,
}

/// Read one coherent board snapshot without reconciling or accepting the Plan.
///
/// # Errors
/// Rejects non-trackers, missing members or dependencies, cycles, and invalid membership.
pub fn plan_progress(ctx: &Ctx, id: &str) -> Result<PlanProgress> {
    let _board = ctx
        .store
        .try_lock("dispatch")?
        .context("board is being updated; retry Plan progress")?;
    let store = Store::open_unobserved(ctx.store.root())?;
    let tracker = store.get(id)?;
    ensure!(tracker.tracker, "{} is not a tracker Plan", tracker.id);
    let fresh = Ctx {
        store: store.clone(),
        ..ctx.clone()
    };
    let mut work = Vec::new();
    let mut counts = PlanWorkCounts::default();
    let mut coverage: Vec<_> = tracker
        .approved_scope
        .iter()
        .flat_map(|scope| &scope.acceptance)
        .map(|acceptance| PlanCoverage {
            acceptance: acceptance.clone(),
            tasks: Vec::new(),
        })
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for id in &tracker.children {
        ensure!(seen.insert(id), "duplicate Plan member {id}");
        let child = store
            .get(id)
            .with_context(|| format!("{}: missing Plan member {id}", tracker.id))?;
        if tracker.approved_scope.is_some() {
            ensure!(
                store.execution_plan(&child)?.as_ref().map(|plan| &plan.id) == Some(&tracker.id),
                "{id}: execution Plan membership changed"
            );
        }
        // Validate the complete reachable graph even for accepted or dropped work.
        crate::queue::dependency_check(&store, &child)?;
        let prerequisites = target::prerequisites(&store, &tracker, &child)?;
        let (state, reason) = match child.status {
            Status::Done => (PlanWorkState::Accepted, None),
            Status::Running => (PlanWorkState::Running, None),
            Status::Review => (PlanWorkState::Review, None),
            Status::Dropped => (
                PlanWorkState::Missing,
                Some("required work was dropped; approved scope is unchanged".into()),
            ),
            Status::NeedsMe => (PlanWorkState::Waiting, Some("owner input required".into())),
            Status::Idea | Status::Backlog | Status::Ready => {
                match crate::operating::queued_origin(&store, &child) {
                    Err(error) => (PlanWorkState::Missing, Some(format!("{error:#}"))),
                    Ok(_) => {
                        let reason = prerequisites
                            .iter()
                            .find(|dependency| !dependency.satisfied)
                            .and_then(|dependency| dependency.reason.clone())
                            .or(crate::queue::blocked_reason(&fresh, &store, &child, false)?);
                        (
                            PlanWorkState::Waiting,
                            Some(reason.unwrap_or_else(|| "ready for authorized dispatch".into())),
                        )
                    }
                }
            }
        };
        match state {
            PlanWorkState::Accepted => counts.accepted += 1,
            PlanWorkState::Running => counts.running += 1,
            PlanWorkState::Review => counts.review += 1,
            PlanWorkState::Waiting => counts.waiting += 1,
            PlanWorkState::Missing => counts.missing += 1,
        }
        for mapping in &mut coverage {
            if maps_acceptance(&store, &tracker, &child, &mapping.acceptance)? {
                mapping.tasks.push(child.id.clone());
            }
        }
        let run = child.current_run().map(|run| run.id.clone());
        work.push(PlanWork {
            task: child.id,
            title: child.title,
            status: child.status,
            state,
            run,
            reason,
            prerequisites,
        });
    }
    let unmapped_criteria = coverage
        .iter()
        .filter(|mapping| mapping.tasks.is_empty())
        .map(|mapping| mapping.acceptance.clone())
        .collect();
    Ok(PlanProgress {
        task: tracker.id,
        status: tracker.status,
        approved_scope: tracker.approved_scope,
        scope_revision: tracker.approved_scope_revision,
        counts,
        work,
        coverage,
        unmapped_criteria,
    })
}

fn maps_acceptance(
    store: &Store,
    tracker: &Task,
    child: &Task,
    acceptance: &PlanAcceptance,
) -> Result<bool> {
    if child.completion.as_ref().is_some_and(|manifest| {
        manifest.criteria.iter().any(|criterion| {
            acceptance.criterion.as_ref() == Some(&criterion.id)
                && acceptance.scenario == criterion.scenario
                && acceptance.expected == criterion.expected
        })
    }) {
        return Ok(true);
    }
    let Some(link) = tracker
        .approved_scope
        .as_ref()
        .and_then(|scope| scope.approved_design.as_deref())
        .filter(|link| child.plan.as_deref() == Some(*link))
    else {
        return Ok(false);
    };
    let (id, revision) = link
        .split_once('@')
        .context("invalid approved design link")?;
    let design = store.get(id)?;
    let plan = design
        .plans
        .iter()
        .find(|plan| plan.id == revision)
        .context("approved design revision is missing")?;
    let Some(scope) = plan
        .document
        .dependencies
        .iter()
        .find(|scope| scope.task == child.title)
    else {
        return Ok(false);
    };
    let scenarios = if scope.acceptance.is_empty() {
        &plan.document.acceptance
    } else {
        &scope.acceptance
    };
    Ok(scenarios.contains(acceptance))
}

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
    scope.validate(store, project)?;
    validate_target(&scope.delivery_target)?;
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

#[cfg(test)]
#[path = "tracker_progress_tests.rs"]
mod progress_tests;
