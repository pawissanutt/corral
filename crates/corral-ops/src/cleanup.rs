//! Best-effort terminal cleanup. Ownership comes from saved dispatches and creation events,
//! never a pane title, cwd, or workspace label.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::{Run, Status, Task};
use serde_json::json;

use crate::Ctx;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cleanup {
    Task,
    Round,
}

/// Terminal tasks release all runs; send-back only releases the current run's review tools.
/// Keep records intact so sync can retry failures and reap resources left by older versions.
pub(crate) fn cleanup(ctx: &Ctx, task: &Task, scope: Cleanup) {
    if let Err(error) = clean(ctx, std::iter::once(task), scope) {
        failed(ctx, task, "inventory", &error);
    }
}

/// One inventory for the whole sweep, rather than subprocesses per finished task on each board tick.
pub(crate) fn sweep(ctx: &Ctx, tasks: &[Task]) {
    let terminal: Vec<&Task> = tasks
        .iter()
        .filter(|task| matches!(task.status, Status::Done | Status::Dropped))
        .collect();
    if let Err(error) = clean(ctx, terminal.iter().copied(), Cleanup::Task)
        && let Some(task) = terminal.first()
    {
        failed(ctx, task, "inventory", &error);
    }
}

fn failed(ctx: &Ctx, task: &Task, resource: &str, error: &anyhow::Error) {
    // Cleanup must not fail the lifecycle action or write over the board's terminal.
    let _ = ctx.store.log_event(
        Some(&task.id),
        "cleanup_failed",
        json!({ "resource": resource, "error": format!("{error:#}") }),
    );
}

fn review_panes<'a>(run: &'a Run, panes: &mut BTreeSet<&'a str>) {
    if let Some(pane) = run.gate.as_ref().and_then(|g| g.pane.as_deref()) {
        panes.insert(pane);
    }
    if let Some(review) = &run.review
        && review.reviewer != "pm"
        && let Some(pane) = review.pane.as_deref()
    {
        panes.insert(pane);
    }
    for child in &run.review_runs {
        review_panes(child, panes);
    }
}

fn clean<'a>(ctx: &Ctx, tasks: impl Iterator<Item = &'a Task>, scope: Cleanup) -> Result<()> {
    let mut owned = BTreeMap::new();
    let mut workspaces = BTreeMap::new();
    // Gate records are replaced each round. Creation events retain earlier panes, including
    // ones whose previous close failed, and record fallback workspace ownership explicitly.
    let events = ctx.store.events()?;
    for task in tasks {
        let runs = match scope {
            Cleanup::Task => task.runs.as_slice(),
            Cleanup::Round => task.runs.last().map(std::slice::from_ref).unwrap_or(&[]),
        };
        let mut panes = BTreeSet::new();
        if scope == Cleanup::Task {
            for review in task.plans.iter().flat_map(|plan| &plan.reviews) {
                if review.reviewer != "pm"
                    && let Some(h) = &review.herdr
                {
                    panes.insert(h.pane_id.as_str());
                    workspaces.insert(h.workspace_id.as_str(), task);
                }
            }
        }
        for run in runs {
            review_panes(run, &mut panes);
            if scope == Cleanup::Task
                && let Some(h) = &run.herdr
            {
                panes.insert(h.pane_id.as_str());
                // Only top-level refs are dispatch-created, unlike PM accounting children.
                workspaces.insert(h.workspace_id.as_str(), task);
            }
        }
        for event in &events {
            if event.task.as_deref() != Some(task.id.as_str())
                || !runs
                    .iter()
                    .any(|run| event.detail["run"].as_str() == Some(&run.id))
            {
                continue;
            }
            let is_tool = event.kind == "gate_started"
                || (event.kind == "review_started"
                    && event.detail["reviewer"].as_str().is_some_and(|r| r != "pm"));
            if (is_tool || event.kind == "run_workspace_created")
                && let Some(pane) = event.detail["pane"].as_str()
            {
                panes.insert(pane);
            }
            if scope == Cleanup::Task
                && event.kind == "run_workspace_created"
                && let Some(workspace) = event.detail["workspace"].as_str()
            {
                workspaces.insert(workspace, task);
            }
        }
        owned.extend(panes.into_iter().map(|pane| (pane, task)));
    }
    if owned.is_empty() && workspaces.is_empty() {
        return Ok(());
    }
    let panes = ctx.herdr.pane_list()?;
    for pane in &panes {
        if let Some(task) = owned.get(pane.pane_id.as_str())
            && let Err(error) = ctx.herdr.pane_close(&pane.pane_id)
        {
            failed(ctx, task, &pane.pane_id, &error);
        }
    }
    if scope == Cleanup::Task && !workspaces.is_empty() {
        // Re-read after pane closure: failed closes and user tabs must keep the workspace.
        let remaining = ctx.herdr.pane_list()?;
        for (workspace, _) in ctx.herdr.workspace_list()? {
            if let Some(task) = workspaces.get(workspace.as_str())
                && !remaining.iter().any(|p| p.workspace_id == workspace)
                && let Err(error) = ctx.herdr.workspace_close(&workspace)
            {
                failed(ctx, task, &workspace, &error);
            }
        }
    }
    Ok(())
}
