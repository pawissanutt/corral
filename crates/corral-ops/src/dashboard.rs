//! Recurrence authorizes the ordinary dispatch queue, never a separate execution path.
use anyhow::{Context, Result, bail};
use corral_core::{DashboardSchedule, Status, Task};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::{Ctx, DispatchOpts, sync::Change};

/// Configure (or resume) a schedule. The caller must explicitly authorize dispatch.
pub fn configure(
    ctx: &Ctx,
    id: &str,
    interval_secs: u32,
    next_due: Timestamp,
    authorize_dispatch: bool,
) -> Result<Task> {
    if !authorize_dispatch {
        bail!("a dashboard schedule requires --authorize-dispatch");
    }
    if interval_secs == 0 {
        bail!("interval seconds must be greater than zero");
    }
    let task = ctx.store.update(id, |task| {
        if task.project.is_none() || task.status == Status::Dropped {
            bail!("a dashboard needs a project and a non-Dropped task");
        }
        task.dashboard = Some(DashboardSchedule {
            interval_secs,
            next_due,
            paused: false,
        });
        // Keep project-level admission from launching the first report before it is due.
        // Explicitly queued/running work retains its existing authorization.
        if !task.queued && task.launch.is_none() && task.runs.is_empty() {
            task.dispatch_hold = true;
        }
        ctx.store.log_event(
            Some(&task.id),
            "dashboard_authorized",
            json!({
                "interval_secs": interval_secs, "next_due": next_due.to_string(),
                "automatic_dispatch": true,
            }),
        )?;
        Ok(())
    })?;
    Ok(task)
}

/// Pause or remove future recurrence; an already authorized queue/run is unchanged.
pub fn disable(ctx: &Ctx, id: &str, remove: bool) -> Result<Task> {
    let task = ctx.store.update(id, |task| {
        if remove {
            task.dashboard = None;
        } else {
            task.dashboard
                .as_mut()
                .context("task has no dashboard schedule")?
                .paused = true;
        }
        ctx.store.log_event(
            Some(&task.id),
            "dashboard_disabled",
            json!({"removed": remove}),
        )?;
        Ok(())
    })?;
    Ok(task)
}

fn eligible(task: &Task, now: Timestamp) -> bool {
    task.dashboard
        .as_ref()
        .is_some_and(|s| !s.paused && s.next_due <= now)
        && !task.queued
        && task.launch.is_none()
        && task.review_reservation.is_none()
        && task.review_wait.is_none()
        && task.current_run().is_none_or(|r| r.outcome.is_some())
        && (task.status == Status::Done
            || (task.runs.is_empty() && matches!(task.status, Status::Backlog | Status::Ready)))
}

/// Queue due occurrences under the same launch and board locks as ordinary dispatch.
/// Active, failed, review and attention states are never recycled automatically.
pub(crate) fn advance(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for task in due_candidates(ctx, now)? {
        match queue_due(ctx, &task.id, now) {
            Ok(Some(change)) => changes.push(change),
            Ok(None) => {}
            Err(err) => crate::messages::report(
                Some(&task.id),
                "dashboard",
                format!("dashboard {}: {err:#}", task.id),
            ),
        }
    }
    Ok(changes)
}

/// The tasks whose occurrence is due: only an unpaused schedule due by `now` can be eligible,
/// so the `tasks_dashboard_due` page bounds the candidates.
fn due_candidates(ctx: &Ctx, now: Timestamp) -> Result<Vec<Task>> {
    let mut due = Vec::new();
    for head in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::DashboardDue { now })?
    {
        let task = head.to_shell_task()?;
        if eligible(&task, now) {
            due.push(task);
        }
    }
    Ok(due)
}

fn queue_due(ctx: &Ctx, id: &str, now: Timestamp) -> Result<Option<Change>> {
    let Some(_launch) = ctx.store.try_lock(&format!("launch-{id}"))? else {
        return Ok(None);
    };
    let task = ctx.store.get(id)?;
    if !eligible(&task, now) {
        return Ok(None);
    }
    if crate::operating::require(
        &ctx.store,
        &task,
        corral_core::OperatingActionKind::Dispatch,
    )
    .is_err()
    {
        return Ok(None);
    }
    let mut candidate = task.clone();
    candidate.status = Status::Ready;
    let opts = DispatchOpts::default();
    if let Some(reason) = crate::queue::pending_plan_reason(&candidate)? {
        bail!("{reason}");
    }
    let plan = crate::queue::resolve_plan(ctx, &candidate, &opts)?;
    if !crate::plan_dispatch::needed(ctx, &candidate, &opts) {
        let approved = crate::approved_plan(ctx, &candidate)?;
        crate::dispatch::check_plan(ctx, &candidate, &plan.route, approved.as_ref())?;
    }
    let (_, changed) = ctx.store.dispatch_update(id, |store, current| {
        if current.updated != task.updated || !eligible(current, now) {
            return Ok(false);
        }
        crate::operating::require(store, current, corral_core::OperatingActionKind::Dispatch)?;
        crate::queue::dependency_check(store, current)?;
        let schedule = current
            .dashboard
            .as_mut()
            .context("missing dashboard schedule")?;
        if schedule.interval_secs == 0 {
            bail!("interval seconds must be greater than zero");
        }
        // Fixed-delay cadence from admission: no catch-up burst after downtime.
        schedule.next_due =
            now.checked_add(SignedDuration::from_secs(i64::from(schedule.interval_secs)))?;
        current.status = Status::Ready;
        current.queued = true;
        current.dispatch_plan = Some(plan.clone());
        current.rounds = 0;
        ctx.store.log_event(
            Some(id),
            "dashboard_queued",
            json!({
                "at": now.to_string(), "schedule": current.dashboard,
            }),
        )?;
        Ok(true)
    })?;
    if !changed {
        return Ok(None);
    }
    Ok(Some(Change {
        task: id.to_string(),
        from: task.status,
        to: Status::Ready,
        note: "scheduled dashboard queued".into(),
        stage: None,
    }))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod hot_tests;
