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
    cleanup_report(ctx, task, scope);
}

pub(crate) fn cleanup_report(ctx: &Ctx, task: &Task, scope: Cleanup) -> Vec<String> {
    match clean(ctx, std::iter::once(task), scope) {
        Ok(warnings) => warnings,
        Err(error) => {
            failed(ctx, task, "inventory", &error);
            vec![format!("pane inventory: {error:#}")]
        }
    }
}

/// One inventory for the whole sweep, rather than subprocesses per finished task on each board tick.
pub(crate) fn sweep(ctx: &Ctx, tasks: &[Task]) {
    // Share one event snapshot across the board, not one read per historical task.
    if tasks.iter().any(has_closed_reviews) {
        match ctx.store.events_shared() {
            Ok(events) => {
                for task in tasks.iter().filter(|task| has_closed_reviews(task)) {
                    reap_with_events(ctx, task, &events);
                }
            }
            Err(error) => {
                if let Some(task) = tasks.first() {
                    failed(ctx, task, "review event inventory", &error);
                }
            }
        }
    }
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

fn has_closed_reviews(task: &Task) -> bool {
    task.runs
        .iter()
        .any(|run| run.review_runs.iter().any(|child| child.ended.is_some()))
}

pub(crate) fn reap_closed_reviews(ctx: &Ctx, task: &Task) {
    if !has_closed_reviews(task) {
        return;
    }
    let events = match ctx.store.events_shared() {
        Ok(events) => events,
        Err(error) => {
            failed(ctx, task, "review event inventory", &error);
            return;
        }
    };
    reap_with_events(ctx, task, &events);
}

fn reap_with_events(ctx: &Ctx, task: &Task, events: &[corral_core::Event]) {
    // The verdict state precedes its event. A concurrent sync must not terminate the
    // reviewer while its verdict command is still writing that event.
    for run in &task.runs {
        for child in run.review_runs.iter().filter(|child| child.ended.is_some()) {
            if let Some(review) = child.review.as_ref().filter(|r| r.verdict.is_some())
                && !events.iter().any(|event| {
                    event.task.as_deref() == Some(&task.id)
                        && event.kind == "verdict"
                        && event.detail["run"].as_str() == Some(&run.id)
                        && event.detail["reviewer"].as_str() == Some(&review.reviewer)
                        && event.ts >= review.at
                })
            {
                continue;
            }
            if let Err(error) = crate::cleanup_worktree::reap_review(ctx, task, run, child) {
                failed(
                    ctx,
                    task,
                    &format!("review {}-{}", run.id, child.id),
                    &error,
                );
            }
        }
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

fn clean<'a>(
    ctx: &Ctx,
    tasks: impl Iterator<Item = &'a Task>,
    scope: Cleanup,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let mut owned = BTreeMap::new();
    let mut review_pane_ids = BTreeSet::new();
    let mut workspaces = BTreeMap::new();
    // Gate records are replaced each round. Creation events retain earlier panes, including
    // ones whose previous close failed, and record fallback workspace ownership explicitly.
    let events = ctx.store.events_shared()?;
    // Grouped once, in log order: a sweep visits every finished task (T-547).
    let mut by_task: BTreeMap<&str, Vec<&corral_core::Event>> = BTreeMap::new();
    for event in events.iter() {
        if let Some(task) = event.task.as_deref() {
            by_task.entry(task).or_default().push(event);
        }
    }
    for task in tasks {
        if !crate::session_scope::owns_task(ctx, task)? {
            warnings.push(
                "pane cleanup refused: selected herdr session does not own task panes".into(),
            );
            continue;
        }
        // Pi-native runtime directories go; recovery-critical sessions stay as transcripts.
        if crate::agent::hooks::task_is_pi_native(task)
            && let Err(error) = crate::agent::hooks::cleanup::run_end(ctx, task)
        {
            failed(ctx, task, "agent-runtime", &error);
            warnings.push(format!("agent runtime: {error:#}"));
        }
        let runs = match scope {
            Cleanup::Task => task.runs.as_slice(),
            Cleanup::Round => task.runs.last().map(std::slice::from_ref).unwrap_or(&[]),
        };
        let mut panes = BTreeSet::new();
        let pending: BTreeSet<&str> = task
            .plans
            .iter()
            .flat_map(|plan| &plan.reviews)
            .filter(|review| review.verdict.is_none())
            .filter_map(|review| review.herdr.as_ref().map(|h| h.pane_id.as_str()))
            .collect();
        if scope == Cleanup::Task {
            for review in task.plans.iter().flat_map(|plan| &plan.reviews) {
                if review.reviewer != "pm"
                    && review.verdict.is_some()
                    && let Some(h) = &review.herdr
                {
                    panes.insert(h.pane_id.as_str());
                    review_pane_ids.insert(h.pane_id.as_str());
                    workspaces.insert(h.workspace_id.as_str(), task);
                }
            }
        }
        for run in runs {
            crate::watch::capture_review(ctx, task, run);
            review_panes(run, &mut panes);
            if let Some(review) = &run.review
                && review.reviewer != "pm"
                && let Some(pane) = review.pane.as_deref()
            {
                review_pane_ids.insert(pane);
            }
            if scope == Cleanup::Task
                && let Some(h) = &run.herdr
            {
                panes.insert(h.pane_id.as_str());
                // Only top-level refs are dispatch-created, unlike PM accounting children.
                workspaces.insert(h.workspace_id.as_str(), task);
            }
        }
        for event in by_task.get(task.id.as_str()).into_iter().flatten() {
            if !runs
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
                if event.kind == "review_started" {
                    review_pane_ids.insert(pane);
                }
            }
            if scope == Cleanup::Task
                && event.kind == "run_workspace_created"
                && let Some(workspace) = event.detail["workspace"].as_str()
            {
                workspaces.insert(workspace, task);
            }
        }
        // Workspace-creation events also include recovered workers. Their saved dispatch
        // reference is authoritative: round cleanup releases tools, not the current worker.
        if scope == Cleanup::Round
            && let Some(worker) = task.current_run().and_then(|run| run.herdr.as_ref())
        {
            panes.remove(worker.pane_id.as_str());
        }
        for pane in pending {
            panes.remove(pane);
        }
        owned.extend(panes.into_iter().map(|pane| (pane, task)));
    }
    if owned.is_empty() && workspaces.is_empty() {
        return Ok(warnings);
    }
    let panes = ctx.herdr.pane_list()?;
    for pane in &panes {
        if let Some(task) = owned.get(pane.pane_id.as_str()) {
            let exits = crate::cleanup_worktree::pane_exits(ctx, &pane.pane_id);
            if let Err(error) = ctx.herdr.pane_close(&pane.pane_id).and_then(|()| {
                crate::cleanup_worktree::wait_pane_exits(&exits);
                if review_pane_ids.contains(pane.pane_id.as_str()) {
                    ctx.store.log_event(
                        Some(&task.id),
                        "cleanup_review_closed",
                        json!({"pane": pane.pane_id}),
                    )?;
                }
                // A successful explicit close is stronger evidence than a listing miss.
                // Release only the records that owned this pane in our cleanup snapshot.
                ctx.store
                    .update(&task.id, |current| {
                        for run in &mut current.runs {
                            let Some(observed) = task.runs.iter().find(|r| r.id == run.id) else {
                                continue;
                            };
                            if run.gate == observed.gate
                                && run.gate.as_ref().is_some_and(|gate| {
                                    gate.holds_slot() && gate.pane.as_deref() == Some(&pane.pane_id)
                                })
                            {
                                crate::review_budget::finish(
                                    ctx,
                                    run,
                                    crate::review_budget::Kind::Gate,
                                );
                            }
                            if run.review == observed.review
                                && run.review.as_ref().is_some_and(|review| {
                                    review.holds_slot()
                                        && review.pane.as_deref() == Some(&pane.pane_id)
                                })
                            {
                                crate::review_budget::finish(
                                    ctx,
                                    run,
                                    crate::review_budget::Kind::Reviewer,
                                );
                            }
                        }
                        Ok(())
                    })
                    .map(|_| ())
            }) {
                failed(ctx, task, &pane.pane_id, &error);
                warnings.push(format!("pane {}: {error:#}", pane.pane_id));
            }
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
                warnings.push(format!("workspace {workspace}: {error:#}"));
            }
        }
    }
    Ok(warnings)
}
