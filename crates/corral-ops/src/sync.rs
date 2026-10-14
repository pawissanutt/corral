use std::collections::HashMap;

use anyhow::Result;
use corral_core::{Status, Task};
use corral_herdr::{AgentStatus, PaneInfo};
use serde_json::json;

use crate::{Ctx, DispatchOpts, DispatchOutcome, dispatch};

/// One status transition made by sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub task: String,
    pub from: Status,
    pub to: Status,
    pub note: String,
}

/// Reconciles every Running/NeedsMe task's current run with herdr, then starts queued tasks:
/// - fetch `pane_list` once; for each run with a herdr ref record agent_state = herdr status (or "gone");
/// - working -> seen_working=true; NeedsMe -> Running;
/// - blocked -> NeedsMe; idle/done with seen_working -> NeedsMe (the agent stopped without `corral done`);
///   on entering NeedsMe capture blocked_excerpt = last 12 non-empty lines of `pane_read` and send
///   `notify("▲ <id> needs you", <last excerpt line>, true)` (notification errors are ignored);
/// - pane gone -> NeedsMe with excerpt "agent pane is gone";
/// - then, while slots are free, dispatch queued tasks (oldest first) whose dependencies are Done
///   (DispatchOpts with the task's preferred agent, queue_if_blocked=true);
///
/// When herdr is unavailable, returns Ok(vec![]) without touching tasks. Logs one event per change.
pub fn sync(ctx: &Ctx) -> Result<Vec<Change>> {
    if !ctx.herdr.available() {
        return Ok(Vec::new());
    }
    let panes: HashMap<String, PaneInfo> = ctx
        .herdr
        .pane_list()?
        .into_iter()
        .map(|p| (p.pane_id.clone(), p))
        .collect();
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if task.status.holds_slot()
            && let Some(change) = reconcile(ctx, &task, &panes)?
        {
            changes.push(change);
        }
    }
    changes.extend(start_queued(ctx)?);
    Ok(changes)
}

fn reconcile(ctx: &Ctx, task: &Task, panes: &HashMap<String, PaneInfo>) -> Result<Option<Change>> {
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    let Some(pane_id) = run.herdr.as_ref().map(|h| h.pane_id.clone()) else {
        return Ok(None);
    };
    let pane = panes.get(&pane_id);
    let state = pane.map_or("gone", |p| p.status.as_str()).to_string();
    let seen_working = run.seen_working || pane.is_some_and(|p| p.status == AgentStatus::Working);

    let (to, note) = match pane.map(|p| p.status) {
        None => (Status::NeedsMe, "agent pane is gone"),
        Some(AgentStatus::Working) => (Status::Running, "agent is working"),
        Some(AgentStatus::Blocked) => (Status::NeedsMe, "agent is blocked"),
        Some(AgentStatus::Idle | AgentStatus::Done) if seen_working => {
            (Status::NeedsMe, "agent stopped without `corral done`")
        }
        _ => (task.status, ""),
    };
    let entering_needs_me = to == Status::NeedsMe && task.status != Status::NeedsMe;
    let excerpt = if !entering_needs_me {
        None
    } else if pane.is_none() {
        Some("agent pane is gone".to_string())
    } else {
        Some(read_excerpt(ctx, &pane_id))
    };
    if let Some(text) = &excerpt {
        let last = text.lines().last().unwrap_or(note);
        if let Err(err) = ctx
            .herdr
            .notify(&format!("▲ {} needs you", task.id), last, true)
        {
            eprintln!("corral: notification for {} failed: {err:#}", task.id);
        }
    }

    // While still waiting, keep the excerpt current: an agent may print its question just after
    // herdr flags it blocked, so the snapshot taken on entry can miss it.
    let refreshed = if to == Status::NeedsMe && !entering_needs_me && pane.is_some() {
        Some(read_excerpt(ctx, &pane_id))
            .filter(|e| !e.is_empty() && run.blocked_excerpt.as_ref() != Some(e))
    } else {
        None
    };
    let excerpt = excerpt.or(refreshed);

    let unchanged = to == task.status
        && run.agent_state.as_deref() == Some(state.as_str())
        && run.seen_working == seen_working
        && excerpt.is_none();
    if unchanged {
        return Ok(None);
    }
    let from = task.status;
    ctx.store.update(&task.id, |t| {
        if let Some(r) = t.current_run_mut() {
            r.agent_state = Some(state.clone());
            r.seen_working = seen_working;
            if let Some(text) = excerpt.clone() {
                r.blocked_excerpt = Some(text);
            } else if to == Status::Running && from == Status::NeedsMe {
                r.blocked_excerpt = None;
            }
        }
        t.status = to;
        Ok(())
    })?;
    if to == from {
        return Ok(None);
    }
    let kind = if to == Status::NeedsMe {
        "needs_me"
    } else {
        "resumed"
    };
    ctx.store.log_event(
        Some(&task.id),
        kind,
        json!({ "from": from.as_str(), "agent_state": state, "note": note }),
    )?;
    Ok(Some(Change {
        task: task.id.clone(),
        from,
        to,
        note: note.to_string(),
    }))
}

fn read_excerpt(ctx: &Ctx, pane_id: &str) -> String {
    match ctx.herdr.pane_read(pane_id, 80) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            lines[lines.len().saturating_sub(12)..].join("\n")
        }
        Err(err) => format!("(could not read agent pane: {err:#})"),
    }
}

fn start_queued(ctx: &Ctx) -> Result<Vec<Change>> {
    let mut queued: Vec<Task> = ctx
        .store
        .list()?
        .into_iter()
        .filter(|t| t.queued && matches!(t.status, Status::Idea | Status::Backlog | Status::Ready))
        .collect();
    queued.sort_by_key(|t| t.created);
    let mut changes = Vec::new();
    for task in queued {
        let (used, limit) = crate::slots(ctx)?;
        if used >= limit {
            break;
        }
        if !deps_done(ctx, &task)? {
            continue;
        }
        let opts = DispatchOpts {
            agent: task.agent.clone(),
            queue_if_blocked: true,
            ..DispatchOpts::default()
        };
        match dispatch(ctx, &task.id, &opts) {
            Ok(DispatchOutcome::Started { run_id, .. }) => changes.push(Change {
                task: task.id.clone(),
                from: task.status,
                to: Status::Running,
                note: format!("dispatched queued task as {run_id}"),
            }),
            Ok(DispatchOutcome::Queued { .. }) => {}
            Err(err) => {
                eprintln!("corral: dispatching queued {} failed: {err:#}", task.id);
                ctx.store.log_event(
                    Some(&task.id),
                    "dispatch_failed",
                    json!({ "error": format!("{err:#}") }),
                )?;
            }
        }
    }
    Ok(changes)
}

fn deps_done(ctx: &Ctx, task: &Task) -> Result<bool> {
    for dep in &task.depends_on {
        if ctx.store.get(dep)?.status != Status::Done {
            return Ok(false);
        }
    }
    Ok(true)
}
