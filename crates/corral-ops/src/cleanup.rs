//! Best-effort terminal cleanup. Ownership comes from saved dispatches and creation events,
//! never a pane title, cwd, or workspace label.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use corral_core::{Run, RunHead, Status, Task};
use corral_herdr::PaneInfo;
use serde_json::json;

use crate::Ctx;

mod reap;
pub(crate) use reap::reap_closed_reviews;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cleanup {
    Task,
    Round,
}

/// Best-effort cleanup of a head. Terminal tasks release all runs; send-back only releases the
/// current run's review tools. Panes come from the task's unreleased cleanup-ledger rows (plan
/// T-253 section 4, "Pane references and cleanup"), never from a walk of its history or its
/// creation events. A round's agent-runtime and capture inputs are the current run with
/// its current and open review children, the only ones that can still own a live tool.
pub(crate) fn cleanup_head(ctx: &Ctx, head: &corral_core::TaskHead, scope: Cleanup) {
    let result = round_shell(head).and_then(|task| clean(ctx, &[(&task, head)], scope));
    if let Err(error) = result {
        failed(ctx, &head.id, "inventory", &error);
    }
    if scope == Cleanup::Task
        && (head.current_run.is_some() || head.summary.runs != 0)
        && crate::session_scope::owns_head(ctx, head).unwrap_or(false)
        && let Err(error) = crate::sandbox_cleanup::reap_task(ctx, &head.id)
    {
        failed(ctx, &head.id, "sandbox", &error);
    }
}

fn round_shell(head: &corral_core::TaskHead) -> Result<Task> {
    use corral_core::TaskRef as _;
    let mut task = head.shell_task()?;
    let Some(run) = head.current_run.as_ref() else {
        return Ok(task);
    };
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
    Ok(task)
}

/// [`cleanup_report`] of a head, as [`cleanup_head`] reads it.
pub(crate) fn cleanup_report_head(
    ctx: &Ctx,
    head: &corral_core::TaskHead,
    scope: Cleanup,
) -> Vec<String> {
    match round_shell(head) {
        Ok(task) => cleanup_report(ctx, &task, scope),
        Err(error) => {
            failed(ctx, &head.id, "inventory", &error);
            vec![format!("{error:#}")]
        }
    }
}

pub(crate) fn cleanup_report(ctx: &Ctx, task: &Task, scope: Cleanup) -> Vec<String> {
    let result = ctx
        .store
        .head(&task.id)
        .and_then(|head| clean(ctx, &[(task, &head)], scope));
    let mut warnings = match result {
        Ok(warnings) => warnings,
        Err(error) => {
            failed(ctx, &task.id, "inventory", &error);
            vec![format!("pane inventory: {error:#}")]
        }
    };
    if scope == Cleanup::Task {
        reap_sandboxes(ctx, task, &mut warnings);
    }
    warnings
}

/// The per-tick retry net, bounded by what herdr lists rather than by finished history: each
/// listed pane, and each listed workspace without a pane, is looked up among the panes tasks
/// recorded, and only Done or Dropped owners are read and cleaned, with one inventory for the
/// whole sweep.
pub(crate) fn sweep_inventory(ctx: &Ctx, board: &corral_core::Board, live: &[PaneInfo]) {
    reap::reap_current_reviews(ctx);
    let result = leaked_owners(ctx, board, live).and_then(|terminal| {
        let heads = terminal
            .iter()
            .map(|task| ctx.store.head(&task.id))
            .collect::<Result<Vec<_>>>()?;
        let owners: Vec<_> = terminal.iter().zip(&heads).collect();
        clean_listed(ctx, &owners, Cleanup::Task, Some(live)).map(|_| terminal)
    });
    if let Err(error) = result {
        crate::messages::report(None, "cleanup", format!("pane cleanup: {error:#}"));
    }
}

/// The sweep over an explicit task list, as tests drive it.
#[cfg(test)]
pub(crate) fn sweep(ctx: &Ctx, tasks: &[Task]) {
    for task in tasks {
        reap_closed_reviews(ctx, task);
    }
    let terminal: Vec<&Task> = tasks
        .iter()
        .filter(|task| matches!(task.status, Status::Done | Status::Dropped))
        .collect();
    let heads: Vec<_> = terminal
        .iter()
        .filter_map(|task| ctx.store.head(&task.id).ok())
        .collect();
    let owners: Vec<_> = terminal.iter().copied().zip(&heads).collect();
    if let Err(error) = clean(ctx, &owners, Cleanup::Task)
        && let Some(task) = terminal.first()
    {
        failed(ctx, &task.id, "inventory", &error);
    }
}

fn reap_sandboxes(ctx: &Ctx, task: &Task, warnings: &mut Vec<String>) {
    if task.runs.is_empty() || !crate::session_scope::owns_task(ctx, task).unwrap_or(false) {
        return;
    }
    if let Err(error) = crate::sandbox_cleanup::reap_task(ctx, &task.id) {
        failed(ctx, &task.id, "sandbox", &error);
        warnings.push(format!("sandbox {}: {error:#}", task.id));
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

/// A round releases the slots its current run held for `pane`: the head form of the full
/// release below, on the current run only.
fn release_round(ctx: &Ctx, task: &Task, pane: &str) -> Result<()> {
    let Some(observed) = task.current_run() else {
        return Ok(());
    };
    let now = corral_core::clock::now();
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

/// Closes the scope's unreleased panes that exist and releases the rows it confirmed: each
/// pane it closed, and each one this server no longer lists (a row recorded on another server
/// stays owed). A Task cleanup then closes its workspaces that no longer hold a pane.
fn clean(
    ctx: &Ctx,
    owners: &[(&Task, &corral_core::TaskHead)],
    scope: Cleanup,
) -> Result<Vec<String>> {
    clean_listed(ctx, owners, scope, None)
}

/// [`clean`] that also closes the `live` panes an owner's ledger names, released or not: the
/// sweep's retry net for panes left by an earlier process, bounded by what herdr lists.
fn clean_listed(
    ctx: &Ctx,
    owners: &[(&Task, &corral_core::TaskHead)],
    scope: Cleanup,
    live: Option<&[PaneInfo]>,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    // pane -> (owner index, ledger rows naming it)
    let mut owned: BTreeMap<String, (usize, Vec<corral_core::CleanupPane>)> = BTreeMap::new();
    let mut workspaces: BTreeMap<String, usize> = BTreeMap::new();
    // Workspace debts name no pane: each stays with its own owner, outside the pane map.
    let mut debts: Vec<(usize, corral_core::CleanupPane)> = Vec::new();
    for (i, (task, head)) in owners.iter().enumerate() {
        if !crate::session_scope::owns_head(ctx, head)? {
            warnings.push(
                "pane cleanup refused: selected herdr session does not own task panes".into(),
            );
            continue;
        }
        let pending_reviews = head.pending_plan_review_panes()?;
        // Promote closed containers before retiring their runtime/panes. Provider files stay.
        if let Err(error) = crate::agent::hooks::cleanup::run_end(ctx, task) {
            failed(ctx, &task.id, "agent-runtime", &error);
            warnings.push(format!("agent runtime: {error:#}"));
        }
        let runs = match scope {
            Cleanup::Task => task.runs.as_slice(),
            Cleanup::Round => task.runs.last().map(std::slice::from_ref).unwrap_or(&[]),
        };
        for run in runs {
            crate::watch::capture_review(ctx, *task, &RunHead::of(run)?);
        }
        let mut rows = head.unreleased_panes(scope == Cleanup::Round)?;
        for listed in live.unwrap_or_default() {
            for row in head.ledger_named(&listed.pane_id)? {
                if !rows.iter().any(|r| r.key == row.key) {
                    rows.push(row);
                }
            }
        }
        for pane in rows {
            if pending_reviews.contains(&pane.pane) {
                continue;
            }
            if scope == Cleanup::Round
                && head
                    .current_run
                    .as_ref()
                    .and_then(|run| run.herdr.as_ref())
                    .is_some_and(|worker| worker.pane_id == pane.pane)
            {
                continue;
            }
            let tool = matches!(
                pane.kind.as_str(),
                "gate" | "review" | "plan_review" | "workspace"
            );
            if scope == Cleanup::Round && !tool {
                continue;
            }
            if scope == Cleanup::Task
                && let Some(workspace) = &pane.workspace
            {
                workspaces.insert(workspace.clone(), i);
            }
            if pane.pane.is_empty() {
                debts.push((i, pane));
                continue;
            }
            owned
                .entry(pane.pane.clone())
                .or_insert_with(|| (i, Vec::new()))
                .1
                .push(pane);
        }
    }
    if owned.is_empty() && workspaces.is_empty() && debts.is_empty() {
        return Ok(warnings);
    }
    let listed = ctx.herdr.pane_list()?;
    let socket = ctx
        .herdr
        .socket_path()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Rows whose pane this pass closed or found absent. A Round release leaves the row's
    // workspace owed to a Task cleanup; a Task cleanup keeps a row whose workspace stays open.
    let mut done: Vec<(usize, corral_core::CleanupPane)> = debts
        .into_iter()
        .filter(|(_, r)| r.socket.is_empty() || r.socket == socket)
        .collect();
    for (pane, (i, rows)) in &owned {
        let (task, _) = owners[*i];
        match listed.iter().find(|p| &p.pane_id == pane) {
            Some(_) => {
                // A successful explicit close is stronger evidence than a listing miss.
                // Release only the slots that owned this pane in our cleanup snapshot.
                let exits = crate::cleanup_worktree::pane_exits(ctx, pane);
                match ctx.herdr.pane_close(pane).and_then(|()| {
                    crate::cleanup_worktree::wait_pane_exits(&exits);
                    if rows
                        .iter()
                        .any(|row| matches!(row.kind.as_str(), "review" | "plan_review"))
                    {
                        ctx.store.log_event(
                            Some(&task.id),
                            "cleanup_review_closed",
                            json!({"pane": pane}),
                        )?;
                    }
                    release_round(ctx, task, pane)
                }) {
                    Ok(()) => done.extend(rows.iter().map(|r| (*i, r.clone()))),
                    Err(error) => {
                        failed(ctx, &task.id, pane, &error);
                        warnings.push(format!("pane {pane}: {error:#}"));
                    }
                }
            }
            None => done.extend(
                rows.iter()
                    .filter(|r| r.socket.is_empty() || r.socket == socket)
                    .map(|r| (*i, r.clone())),
            ),
        }
    }
    let mut gone: Option<BTreeSet<String>> = None;
    if scope == Cleanup::Task && !workspaces.is_empty() {
        // Re-read after pane closure: failed closes and user tabs must keep the workspace.
        let remaining = ctx.herdr.pane_list()?;
        let mut open = BTreeSet::new();
        for (workspace, _) in ctx.herdr.workspace_list()? {
            if let Some(&i) = workspaces.get(&workspace)
                && !remaining.iter().any(|p| p.workspace_id == workspace)
            {
                if let Err(error) = ctx.herdr.workspace_close(&workspace) {
                    failed(ctx, &owners[i].0.id, &workspace, &error);
                    warnings.push(format!("workspace {workspace}: {error:#}"));
                    open.insert(workspace);
                }
            } else {
                open.insert(workspace);
            }
        }
        gone = Some(open);
    }
    // owner -> (released keys, workspaces they leave owed)
    type Settled = (Vec<String>, Vec<(String, String)>);
    let mut released: BTreeMap<usize, Settled> = BTreeMap::new();
    for (i, row) in done {
        let entry = released.entry(i).or_default();
        match (&row.workspace, &gone) {
            (None, _) => entry.0.push(row.key),
            (Some(workspace), Some(open)) => {
                if !open.contains(workspace) {
                    entry.0.push(row.key);
                }
            }
            (Some(workspace), None) => {
                entry.0.push(row.key);
                entry.1.push((workspace.clone(), row.socket.clone()));
            }
        }
    }
    for (i, (keys, owed)) in released {
        if keys.is_empty() {
            continue;
        }
        let (task, _) = owners[i];
        if let Err(error) = ctx
            .store
            .update_head(&task.id, |head| head.release_panes(&keys, &owed))
        {
            failed(ctx, &task.id, "inventory", &error);
        }
    }
    Ok(warnings)
}

#[cfg(all(test, unix))]
#[path = "../tests/support/cleanup_ledger.rs"]
mod ledger_tests;

/// Exact task transition owned by publishing a newly allocated fallback workspace.
pub(crate) struct WorkspacePublication {
    pub before: corral_core::TaskHead,
    pub after: corral_core::TaskHead,
}

/// Records a run's fallback workspace in the cleanup ledger, in the operation that logs it.
pub(crate) fn record_run_workspace(
    ctx: &Ctx,
    task: &str,
    run: &str,
    pane: &str,
    workspace: &str,
) -> Result<WorkspacePublication> {
    let socket = ctx.herdr.socket_path()?;
    let mut before = None;
    let after = ctx.store.update_head(task, |head| {
        before = Some(head.head().clone());
        if head
            .head()
            .current_run
            .as_ref()
            .is_some_and(|r| r.id == run)
        {
            head.record_run_workspace(run, pane, workspace, &socket.to_string_lossy())?;
        }
        ctx.store.log_event(
            Some(task),
            "run_workspace_created",
            json!({ "run": run, "workspace": workspace, "pane": pane }),
        )
    })?;
    Ok(WorkspacePublication {
        before: before.context("workspace publication did not observe its task")?,
        after,
    })
}
