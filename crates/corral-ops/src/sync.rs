use std::collections::HashMap;

use anyhow::Result;
use corral_core::{AutoStage, Run, Status, Task};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::{Ctx, DispatchOpts, DispatchOutcome, dispatch, omo};

/// One status transition made by sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub task: String,
    pub from: Status,
    pub to: Status,
    pub note: String,
    /// The autopilot stage this change entered; None for herdr reconciliation and dispatch.
    pub stage: Option<AutoStage>,
}

/// Reconciles every Running/NeedsMe task's current run with herdr, then starts queued tasks:
/// - fetch `pane_list` once; for each run with a herdr ref record agent_state = herdr status (or "gone");
/// - working -> seen_working=true; NeedsMe -> Running;
/// - blocked -> NeedsMe; on entering NeedsMe capture blocked_excerpt = last 12 non-empty lines of
///   `pane_read` and send `notify("▲ <id> needs you", <last excerpt line>, true)` (notification errors
///   are ignored);
/// - pane gone -> NeedsMe with excerpt "agent pane is gone";
/// - records finished gates (`gate_collect`) of runs whose gate is still marked running;
/// - advances the review autopilot of every Review task (gate, reviewer, land or loop back; `[review]
///   autopilot` off stops it), reporting its status moves and stage transitions as changes;
/// - idle/done without `corral done` never moves the card (docs/DESIGN.md §5): once seen_working, a
///   Running run records idle_since on the first idle sample and, after `[sync] idle_after_secs`,
///   sets idle_no_report and logs one "idle_no_report" event (no notification); activity clears both;
/// - OmO runs: herdr misdetects OmO, so an idle/done run whose newest OmO session file changed since
///   the run started and within the idle window counts as working (agent_state "working");
/// - then, while slots are free, dispatch queued tasks (oldest first) whose dependencies are Done
///   (DispatchOpts with the task's preferred agent, queue_if_blocked=true);
///
/// When herdr is unavailable, returns Ok(vec![]) without touching tasks. Logs one event per change.
pub fn sync(ctx: &Ctx) -> Result<Vec<Change>> {
    sync_at(ctx, Timestamp::now())
}

/// `sync` with an explicit clock; `now` drives the idle debounce.
pub fn sync_at(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
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
            && let Some(change) = reconcile(ctx, &task, &panes, now)?
        {
            changes.push(change);
        }
    }
    crate::gate::collect_finished(ctx)?;
    changes.extend(crate::autopilot::advance_all(ctx, now)?);
    changes.extend(start_queued(ctx)?);
    Ok(changes)
}

fn reconcile(
    ctx: &Ctx,
    task: &Task,
    panes: &HashMap<String, PaneInfo>,
    now: Timestamp,
) -> Result<Option<Change>> {
    if !task.status.holds_slot() {
        return Ok(None);
    }
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    let Some(pane_id) = run.herdr.as_ref().map(|h| h.pane_id.clone()) else {
        return Ok(None);
    };
    let pane = panes.get(&pane_id);
    let herdr_status = pane.map(|p| p.status);
    let looks_idle = matches!(herdr_status, Some(AgentStatus::Idle | AgentStatus::Done));
    let idle_after = SignedDuration::from_secs(
        i64::try_from(ctx.store.config.sync.idle_after_secs).unwrap_or(i64::MAX),
    );
    let omo_active = looks_idle && omo_recently_active(ctx, run, now, idle_after);
    let working = herdr_status == Some(AgentStatus::Working) || omo_active;
    let state = match pane {
        None => "gone".to_string(),
        Some(_) if omo_active => AgentStatus::Working.as_str().to_string(),
        Some(p) => p.status.as_str().to_string(),
    };
    let seen_working = run.seen_working || working;

    let (to, note) = match herdr_status {
        None => (Status::NeedsMe, "agent pane is gone"),
        Some(AgentStatus::Working) => (Status::Running, "agent is working"),
        Some(AgentStatus::Blocked) => (Status::NeedsMe, "agent is blocked"),
        _ => (task.status, ""),
    };
    let (idle_since, idle_no_report) = if to != Status::Running || working {
        (None, false)
    } else if looks_idle && seen_working {
        let since = run.idle_since.unwrap_or(now);
        let flagged = run.idle_no_report || now.duration_since(since) >= idle_after;
        (Some(since), flagged)
    } else {
        (run.idle_since, run.idle_no_report)
    };
    let newly_flagged = idle_no_report && !run.idle_no_report;

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
        && run.idle_since == idle_since
        && run.idle_no_report == idle_no_report
        && excerpt.is_none();
    if unchanged {
        return Ok(None);
    }
    let from = task.status;
    let run_id = run.id.clone();
    // Everything above was decided from the snapshot `sync` listed; herdr calls since then leave time
    // for a CLI writer (`corral done`, move, retry, ...). Apply only if the task is still in the state
    // we reconciled: same status, same run, run still open. Otherwise skip; the next tick re-reads.
    let written = ctx.store.try_update(&task.id, |t| {
        if t.status != from {
            return Ok(false);
        }
        let Some(r) = t
            .current_run_mut()
            .filter(|r| r.id == run_id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        r.agent_state = Some(state.clone());
        r.seen_working = seen_working;
        r.idle_since = idle_since;
        r.idle_no_report = idle_no_report;
        if let Some(text) = excerpt.clone() {
            r.blocked_excerpt = Some(text);
        } else if to == Status::Running && from == Status::NeedsMe {
            r.blocked_excerpt = None;
        }
        t.status = to;
        Ok(true)
    })?;
    if written.is_none() {
        return Ok(None);
    }
    if newly_flagged {
        ctx.store.log_event(
            Some(&task.id),
            "idle_no_report",
            json!({
                "agent_state": state,
                "idle_since": idle_since.map(|t| t.to_string()),
                "note": "agent looks idle without `corral done`",
            }),
        )?;
    }
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
        stage: None,
    }))
}

/// OmO runs only: the newest session file for the run's worktree changed after the run started and
/// less than `window` before `now`.
fn omo_recently_active(ctx: &Ctx, run: &Run, now: Timestamp, window: SignedDuration) -> bool {
    if !omo::is_omo(run) {
        return false;
    }
    let agent = omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
    omo::last_activity(&agent, &run.worktree)
        .is_some_and(|t| t >= run.started && now.duration_since(t) < window)
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
            profile: task.profile.clone(),
            queue_if_blocked: true,
            ..DispatchOpts::default()
        };
        match dispatch(ctx, &task.id, &opts) {
            Ok(DispatchOutcome::Started { run_id, .. }) => changes.push(Change {
                task: task.id.clone(),
                from: task.status,
                to: Status::Running,
                note: format!("dispatched queued task as {run_id}"),
                stage: None,
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
