//! Best-effort terminal cleanup. Ownership comes from saved dispatches and creation events,
//! never a pane title, cwd, or workspace label.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::{Run, Status, Task};
use corral_herdr::PaneInfo;
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

/// [`cleanup`] of a head. A round reads only the current run's subtree that can still own a
/// live tool pane (the run itself, its current child and its open children) and that run's own
/// pane-creation events; a task cleanup reads the full task, as Land and approve do.
pub(crate) fn cleanup_head(ctx: &Ctx, head: &corral_core::TaskHead, scope: Cleanup) {
    if scope == Cleanup::Task {
        match ctx.store.get(&head.id) {
            Ok(task) => cleanup(ctx, &task, scope),
            Err(error) => failed(ctx, &head.id, "inventory", &error),
        }
        return;
    }
    if let Err(error) = clean_round_head(ctx, head) {
        failed(ctx, &head.id, "inventory", &error);
    }
}

fn clean_round_head(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<()> {
    use corral_core::TaskRef as _;
    let Some(run) = head.current_run.as_ref() else {
        return Ok(());
    };
    let mut task = head.shell_task()?;
    let mut children: Vec<Run> = Vec::new();
    for child in head
        .open_review_children(&run.id)?
        .iter()
        .chain(run.current_child.as_deref())
    {
        if !children.iter().any(|c| c.id == child.id) {
            children.push(serde_json::from_value(serde_json::to_value(child)?)?);
        }
    }
    if let Some(current) = task.runs.last_mut() {
        current.review_runs = children;
    }
    let events = ctx.store.events_of_run(
        Some(&head.id),
        &["gate_started", "review_started", "run_workspace_created"],
        Some(&run.id),
        0,
        u32::MAX,
    )?;
    clean_with(ctx, std::iter::once(&task), Cleanup::Round, events).map(drop)
}

pub(crate) fn cleanup_report(ctx: &Ctx, task: &Task, scope: Cleanup) -> Vec<String> {
    match clean(ctx, std::iter::once(task), scope) {
        Ok(warnings) => warnings,
        Err(error) => {
            failed(ctx, &task.id, "inventory", &error);
            vec![format!("pane inventory: {error:#}")]
        }
    }
}

/// The per-tick retry net, bounded by what herdr lists rather than by finished history: each
/// listed pane, and each listed workspace without a pane, is looked up among the panes tasks
/// recorded, and only Done or Dropped owners are read and cleaned, with one inventory for the
/// whole sweep. Panes that only a finished task's creation events still name are left to that
/// task's own terminal cleanup.
pub(crate) fn sweep_inventory(ctx: &Ctx, board: &corral_core::Board, live: &[PaneInfo]) {
    let terminal = match leaked_owners(ctx, board, live) {
        Ok(terminal) => terminal,
        Err(error) => {
            crate::messages::report(None, "cleanup", format!("pane owner lookup: {error:#}"));
            return;
        }
    };
    if let Err(error) = clean(ctx, terminal.iter(), Cleanup::Task)
        && let Some(task) = terminal.first()
    {
        failed(ctx, &task.id, "inventory", &error);
    }
}

/// The sweep over an explicit task list, as tests drive it.
#[cfg(test)]
pub(crate) fn sweep(ctx: &Ctx, tasks: &[Task]) {
    let terminal: Vec<&Task> = tasks
        .iter()
        .filter(|task| matches!(task.status, Status::Done | Status::Dropped))
        .collect();
    if let Err(error) = clean(ctx, terminal.iter().copied(), Cleanup::Task)
        && let Some(task) = terminal.first()
    {
        failed(ctx, &task.id, "inventory", &error);
    }
}

fn leaked_owners(ctx: &Ctx, board: &corral_core::Board, live: &[PaneInfo]) -> Result<Vec<Task>> {
    let mut owners = BTreeSet::new();
    for pane in live {
        owners.extend(ctx.store.pane_owners(&format!("pane:{}", pane.pane_id))?);
    }
    let busy: BTreeSet<&str> = live.iter().map(|p| p.workspace_id.as_str()).collect();
    for (workspace, _) in ctx.herdr.workspace_list()? {
        if !busy.contains(workspace.as_str()) {
            owners.extend(ctx.store.pane_owners(&format!("workspace:{workspace}"))?);
        }
    }
    let open: BTreeSet<&str> = board.heads.iter().map(|head| head.id.as_str()).collect();
    let mut terminal = Vec::new();
    for id in owners.iter().filter(|id| !open.contains(id.as_str())) {
        let task = ctx.store.get(id)?;
        if matches!(task.status, Status::Done | Status::Dropped) {
            terminal.push(task);
        }
    }
    Ok(terminal)
}

fn failed(ctx: &Ctx, task: &str, resource: &str, error: &anyhow::Error) {
    // Cleanup must not fail the lifecycle action or write over the board's terminal.
    let _ = ctx.store.log_event(
        Some(task),
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

/// A round releases the slots its current run held for `pane`: the head form of the full
/// release below, on the current run only.
fn release_round(ctx: &Ctx, task: &Task, pane: &str) -> Result<()> {
    let Some(observed) = task.current_run() else {
        return Ok(());
    };
    let now = jiff::Timestamp::now();
    ctx.store
        .try_update_head(&task.id, |current| {
            let Some(run) = current
                .current_run_mut()
                .filter(|run| run.id == observed.id)
            else {
                return Ok(false);
            };
            let gate = run.gate == observed.gate
                && run
                    .gate
                    .as_ref()
                    .is_some_and(|gate| gate.holds_slot() && gate.pane.as_deref() == Some(pane));
            let review = run.review == observed.review
                && run.review.as_ref().is_some_and(|review| {
                    review.holds_slot() && review.pane.as_deref() == Some(pane)
                });
            if gate {
                crate::review_budget::finish_head(current, crate::review_budget::Kind::Gate, now)?;
            }
            if review {
                crate::review_budget::finish_head(
                    current,
                    crate::review_budget::Kind::Reviewer,
                    now,
                )?;
            }
            Ok(gate || review)
        })
        .map(drop)
}

fn clean<'a>(
    ctx: &Ctx,
    tasks: impl Iterator<Item = &'a Task>,
    scope: Cleanup,
) -> Result<Vec<String>> {
    // Gate records are replaced each round. Creation events retain earlier panes, including
    // ones whose previous close failed, and record fallback workspace ownership explicitly.
    let tasks: Vec<_> = tasks.collect();
    let mut events = Vec::new();
    for task in &tasks {
        events.extend(ctx.store.events_of(
            Some(&task.id),
            &["gate_started", "review_started", "run_workspace_created"],
        )?);
    }
    clean_with(ctx, tasks.into_iter(), scope, events)
}

/// [`clean`] with the pane-creation events of `tasks` already read.
fn clean_with<'a>(
    ctx: &Ctx,
    tasks: impl Iterator<Item = &'a Task>,
    scope: Cleanup,
    events: Vec<corral_core::Event>,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let mut owned = BTreeMap::new();
    let mut workspaces = BTreeMap::new();
    let tasks: Vec<_> = tasks.collect();
    for task in tasks {
        // Pi-native runtime directories go; recovery-critical sessions stay as transcripts.
        if crate::agent::hooks::task_is_pi_native(task)
            && let Err(error) = crate::agent::hooks::cleanup::run_end(ctx, task)
        {
            failed(ctx, &task.id, "agent-runtime", &error);
            warnings.push(format!("agent runtime: {error:#}"));
        }
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
            crate::watch::capture_review(ctx, task, run);
            review_panes(run, &mut panes);
            if scope == Cleanup::Task
                && let Some(h) = &run.herdr
            {
                panes.insert(h.pane_id.as_str());
                // Only top-level refs are dispatch-created, unlike PM accounting children.
                workspaces.insert(h.workspace_id.as_str(), task);
            }
        }
        for event in events.iter() {
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
        return Ok(warnings);
    }
    let panes = ctx.herdr.pane_list()?;
    for pane in &panes {
        if let Some(task) = owned.get(pane.pane_id.as_str())
            && let Err(error) = ctx.herdr.pane_close(&pane.pane_id).and_then(|()| {
                // A successful explicit close is stronger evidence than a listing miss.
                // Release only the records that owned this pane in our cleanup snapshot.
                if scope == Cleanup::Round {
                    return release_round(ctx, task, &pane.pane_id);
                }
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
            })
        {
            failed(ctx, &task.id, &pane.pane_id, &error);
            warnings.push(format!("pane {}: {error:#}", pane.pane_id));
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
                failed(ctx, &task.id, &workspace, &error);
                warnings.push(format!("workspace {workspace}: {error:#}"));
            }
        }
    }
    Ok(warnings)
}
