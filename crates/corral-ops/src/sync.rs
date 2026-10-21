use std::collections::HashMap;

use anyhow::Result;
use corral_core::{AutoStage, Run, Status, Task};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::{Ctx, DispatchOpts, DispatchOutcome, ProcessControl, SystemProcesses, omo};

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
/// - pane gone -> NeedsMe after two consecutive misses of a seen pane, or
///   `[sync] startup_grace_secs` (default 30) elapsed without any sighting;
/// - records finished gates (`gate_collect`) of runs whose gate is still marked running;
/// - reaps owned panes and empty workspaces left behind by Done/Dropped tasks;
/// - advances the review autopilot of every Review task (gate, reviewer, land or loop back; `[review]
///   autopilot` off stops it), reporting its status moves and stage transitions as changes;
/// - idle/done without `corral done` never moves the card (docs/DESIGN.md §5): once seen_working, a
///   Running run records idle_since on the first idle sample and, after `[sync] idle_after_secs`,
///   sets idle_no_report and logs one "idle_no_report" event (no notification); activity clears both;
/// - OmO runs: herdr misdetects OmO, so an idle/done run whose newest OmO session file changed since
///   the run started and within the idle window counts as working (agent_state "working");
/// - then admit authorized Ready tasks by priority and age whose dependencies are Done, preserving
///   review headroom, followed by configured Backlog and Idea admission;
///
/// Failed, malformed or empty inventories are UNKNOWN: warn and skip pane reconciliation.
pub fn sync(ctx: &Ctx) -> Result<Vec<Change>> {
    sync_at(ctx, corral_core::clock::now())
}

/// A plugin hook must belong to both the invoking server and the selected client.
/// Config or CORRAL_HERDR_SESSION may retarget a client, but cannot transfer an event.
pub fn sync_hook(ctx: &Ctx) -> Result<Vec<Change>> {
    let has_source = ["HERDR_SOCKET_PATH", "HERDR_SESSION"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|s| !s.is_empty()));
    if has_source {
        let mut source = ctx.clone();
        source.herdr.set_session(None);
        if !crate::session_scope::owns_inventory(&source)? {
            return Ok(Vec::new());
        }
        return sync_passes(
            ctx,
            corral_core::clock::now(),
            &SystemProcesses,
            Some(&source),
        );
    }
    sync(ctx)
}

/// `sync` with an explicit clock; `now` drives the idle debounce.
pub fn sync_at(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    sync_at_with_processes(ctx, now, &SystemProcesses)
}

/// Explicit clock and process boundary for deterministic watchdog tests.
pub fn sync_at_with_processes(
    ctx: &Ctx,
    now: Timestamp,
    processes: &dyn ProcessControl,
) -> Result<Vec<Change>> {
    sync_passes(ctx, now, processes, None)
}

fn sync_passes(
    ctx: &Ctx,
    now: Timestamp,
    processes: &dyn ProcessControl,
    source: Option<&Ctx>,
) -> Result<Vec<Change>> {
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(Vec::new());
    }
    let mut changes = Vec::new();
    let mut visited = std::collections::BTreeSet::new();
    let mut attempted = std::collections::BTreeSet::new();
    loop {
        let before = serde_json::to_vec(&ctx.store.list()?)?;
        if !visited.insert(crate::verification::digest(&before)) {
            break;
        }
        let pass = sync_scoped(ctx, now, processes, source)?;
        let progressed = !pass.is_empty();
        changes.extend(pass);
        let after = serde_json::to_vec(&ctx.store.list()?)?;
        // A concurrent owner's write is not this pass's progress. In
        // particular, landing during rebuildable indexing must not make this
        // same sync consume a new repair round on an owner-ready peer.
        if !progressed || before == after {
            break;
        }
        // Existing executors do the work. Recompute only after durable progress,
        // never await an owner question or poll an unchanged external condition.
        let workset = crate::pm_workset(ctx)?;
        if workset.actions.is_empty() {
            break;
        }
        let mut fresh_action = false;
        for action in &workset.actions {
            if let Some(id) = &action.action.task {
                let task = ctx.store.get(id)?;
                // Terminal samples cannot create another attempt. Only a new
                // action, candidate/receipt, plan, round or policy can continue.
                let key = crate::verification::digest(&serde_json::to_vec(&(
                    &workset.policy_revision,
                    id,
                    action.authority,
                    &action.action.evidence,
                    task.rounds,
                    task.current_run()
                        .map(|run| (&run.id, &run.gate, &run.review)),
                    task.plans.last(),
                ))?);
                fresh_action |= attempted.insert(key);
            }
        }
        if !fresh_action {
            break;
        }
    }
    Ok(changes)
}

fn sync_scoped(
    ctx: &Ctx,
    now: Timestamp,
    processes: &dyn ProcessControl,
    source: Option<&Ctx>,
) -> Result<Vec<Change>> {
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(Vec::new());
    }
    let mut fresh = ctx.clone();
    fresh.reload_config()?;
    let ctx = &fresh;
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(Vec::new());
    }
    ctx.store.check_runtime()?;
    crate::desktop_notifications::reconcile(ctx)?;
    crate::digest::scheduled_digest_at(ctx, now)?;
    let mut changes = crate::queue::migrate_backlog(ctx)?;
    changes.extend(crate::tracker::reconcile(ctx)?);
    let resources = crate::resources::observe(ctx, now)?;
    changes.extend(crate::resource_escalation::reconcile(ctx, &resources, now)?);
    crate::lesson_distill::announce_due(ctx, now)?;
    let herdr_available = ctx.herdr.available();
    if let Err(error) = crate::pm_policy::reconcile_pm_policy(ctx, herdr_available) {
        // Policy-delivery trouble is visible, but cannot stop receipt collection
        // or unrelated already-authorized work.
        crate::messages::report(None, "pm-policy", format!("{error:#}"));
    }
    if !herdr_available {
        crate::hooks::reconcile_post_land(ctx)?;
        crate::gate::collect_finished(ctx, now)?;
        let search_stamp = crate::content_refresh::task_stamp(ctx)?;
        let tasks = ctx.store.list()?;
        for task in &tasks {
            crate::obligations_sync::reconcile_one(ctx, &task.id, now, None)?;
        }
        crate::movement::reconcile(ctx, now, false)?;
        crate::pm_events::push_events(ctx, false)?;
        crate::desktop_notifications::reconcile(ctx)?;
        crate::content_refresh::refresh_after_sync(ctx, &tasks, &search_stamp)?;
        return Ok(changes);
    }
    let Some(inventory) = crate::pane_liveness::inventory(ctx) else {
        return Ok(changes);
    };
    let search_stamp = crate::content_refresh::task_stamp(ctx)?;
    let tasks = ctx.store.list()?;
    if !crate::session_scope::owns_tasks(ctx, &ctx.store, &tasks)? {
        return Ok(changes);
    }
    if let Some(source) = source
        && !crate::session_scope::owns_tasks(source, &ctx.store, &tasks)?
    {
        return Ok(changes);
    }
    crate::hooks::reconcile_post_land(ctx)?;
    crate::gate::collect_finished(ctx, now)?;
    let panes: HashMap<String, PaneInfo> = inventory
        .panes
        .iter()
        .cloned()
        .map(|p| (p.pane_id.clone(), p))
        .collect();
    let snapshot = inventory.snapshot(&panes);
    crate::cleanup::sweep(ctx, &tasks);
    for task in &tasks {
        if let Some(change) =
            crate::obligations_sync::reconcile_one(ctx, &task.id, now, Some(&panes))?
        {
            changes.push(change);
        }
    }
    for task in ctx.store.list()? {
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        crate::plan_pilot::reconcile(ctx, &task, snapshot, now)?;
        let reconciled = ctx.store.get(&task.id)?;
        if reconciled.status != task.status {
            changes.push(Change {
                task: task.id.clone(),
                from: task.status,
                to: reconciled.status,
                note: reconciled
                    .plan_runs
                    .last()
                    .and_then(|r| r.pending_reply.clone())
                    .unwrap_or_else(|| "planner agent state changed".into()),
                stage: None,
            });
        }
        if let Some(change) =
            crate::reviewer::reconcile_review_sampled_at(ctx, &task.id, now, snapshot)?
        {
            changes.push(change);
        }
        if !crate::plan_pilot::handles(&task) && task.status.holds_slot() && task.launch.is_none() {
            if let Some(change) = crate::provider_stall::advance_agent(ctx, &task, &panes, now)? {
                changes.push(change);
                continue;
            }
            if let Some(change) = reconcile(ctx, &task, snapshot, now, processes)? {
                changes.push(change);
            }
        }
    }
    crate::review_budget::reconcile(ctx, snapshot, now)?;
    // Launching queued/autopilot work must acquire an exclusive ownership claim.
    // No consumer below this point receives the sampled pane IDs.
    drop(inventory);
    crate::plan_pilot::advance_all(ctx, now)?;
    changes.extend(crate::autopilot::advance_all(ctx, now)?);
    crate::movement::reconcile(ctx, now, true)?;
    changes.extend(crate::dashboard::advance(ctx, now)?);
    changes.extend(start_queued(ctx, now)?);
    changes.extend(crate::dispatch_policy::advance(ctx, now)?);
    crate::pm_events::push_events(ctx, true)?;
    crate::desktop_notifications::reconcile(ctx)?;
    crate::content_refresh::refresh_after_sync(ctx, &tasks, &search_stamp)?;
    Ok(changes)
}

fn reconcile(
    ctx: &Ctx,
    task: &Task,
    snapshot: crate::pane_liveness::Snapshot<'_>,
    now: Timestamp,
    processes: &dyn ProcessControl,
) -> Result<Option<Change>> {
    let panes = snapshot.panes;
    let _attribution = ctx.store.attribute(corral_core::Actor::System);
    if !task.status.holds_slot() {
        return Ok(None);
    }
    if crate::resource_escalation::holds_attention(ctx, task)? {
        return Ok(None);
    }
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    let Some(pane_id) = run.herdr.as_ref().map(|h| h.pane_id.clone()) else {
        return Ok(None);
    };
    let pane = panes.get(&pane_id);
    // Older runs have no seen_pane field; their recorded real status is still a sighting.
    let seen_pane = run.seen_pane
        || run.seen_working
        || matches!(
            run.agent_state.as_deref(),
            Some("idle" | "working" | "blocked" | "done" | "unknown")
        )
        || pane.is_some();
    let gone = crate::pane_liveness::gone(
        ctx,
        crate::pane_liveness::Sample {
            task,
            pane: Some(&pane_id),
            started: run.started,
            seen: seen_pane,
            present: pane.is_some(),
            at: now,
            ticket: snapshot.ticket,
            count_miss: true,
        },
    )?;
    let herdr_status = pane.map(|p| p.status);
    let looks_idle = matches!(herdr_status, Some(AgentStatus::Idle | AgentStatus::Done));
    let idle_after = SignedDuration::from_secs(
        i64::try_from(ctx.store.config.sync.idle_after_secs).unwrap_or(i64::MAX),
    );
    let omo_active = looks_idle && omo_recently_active(ctx, run, now, idle_after);
    let working = herdr_status == Some(AgentStatus::Working) || omo_active;
    let state = match pane {
        None if !gone => run.agent_state.clone().unwrap_or_else(|| "starting".into()),
        None => "gone".to_string(),
        Some(_) if omo_active => AgentStatus::Working.as_str().to_string(),
        Some(p) => p.status.as_str().to_string(),
    };
    let seen_working = run.seen_working || working;
    let herdr_idle_since = if looks_idle && task.status == Status::Running {
        Some(run.herdr_idle_since.unwrap_or(now))
    } else {
        None
    };
    let stuck_after = SignedDuration::from_secs(
        i64::try_from(ctx.store.config.sync.stuck_after_secs).unwrap_or(i64::MAX),
    );
    let stuck = if herdr_idle_since.is_some_and(|since| now.duration_since(since) > stuck_after) {
        match ctx
            .herdr
            .pane_process_info(&pane_id)
            .and_then(|info| crate::watchdog::foreign_process(ctx, run, &info, processes))
        {
            Ok(process) => process,
            Err(err) => {
                crate::messages::report(
                    Some(&task.id),
                    "watchdog",
                    format!("inspecting {} for stuck processes failed: {err:#}", task.id),
                );
                run.stuck.clone()
            }
        }
    } else {
        None
    };
    let newly_stuck = stuck.is_some() && stuck != run.stuck;

    let stall_key = crate::provider_stall::agent_key(&task.id, run);
    let stall_retrying = crate::provider_stall::retrying(ctx, &stall_key);
    let stall_escalated = crate::provider_stall::escalated(ctx, &stall_key);
    let decision = run.obligations.pending_decision();
    let harness_question = match crate::pm_questions::pending(ctx, run) {
        Ok(question) => question,
        Err(error) => {
            crate::messages::report(
                Some(&task.id),
                "harness-question",
                format!("cannot inspect pending questions: {error:#}"),
            );
            return Ok(None);
        }
    };
    let waiting = run
        .obligations
        .waits
        .iter()
        .any(|w| w.observation.state == corral_core::WaitState::Pending);
    let (to, note) = if decision.is_some() || harness_question.is_some() {
        (Status::NeedsMe, "owner decision pending")
    } else if stall_escalated && !working {
        (Status::NeedsMe, "provider stall escalated")
    } else if waiting {
        (Status::Running, "waiting for a declared event")
    } else {
        match herdr_status {
            None if !gone => (task.status, ""),
            None => (Status::NeedsMe, "agent pane is gone"),
            Some(AgentStatus::Working) => (Status::Running, "agent is working"),
            Some(_) if stall_retrying => (Status::Running, "provider stalled; retrying"),
            Some(AgentStatus::Blocked) => (Status::NeedsMe, "agent is blocked"),
            _ => (task.status, ""),
        }
    };
    let (idle_since, idle_no_report) = if to != Status::Running || working || waiting {
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
    } else if let Some(d) = decision {
        Some(format!(
            "{}\nAction: {}",
            d.request.question, d.request.requested_action
        ))
    } else if let Some(question) = &harness_question {
        Some(format!(
            "{}\nSource: {}",
            question.question, question.source
        ))
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
            crate::messages::report(
                Some(&task.id),
                "notification",
                format!("notification for {} failed: {err:#}", task.id),
            );
        }
    }

    // While still waiting, keep the excerpt current: an agent may print its question just after
    // herdr flags it blocked, so the snapshot taken on entry can miss it.
    let refreshed = if to == Status::NeedsMe
        && !entering_needs_me
        && pane.is_some()
        && decision.is_none()
        && harness_question.is_none()
        && !stall_escalated
    {
        Some(read_excerpt(ctx, &pane_id))
            .filter(|e| !e.is_empty() && run.blocked_excerpt.as_ref() != Some(e))
    } else {
        None
    };
    let excerpt = excerpt.or(refreshed);

    let unchanged = to == task.status
        && run.agent_state.as_deref() == Some(state.as_str())
        && run.seen_pane == seen_pane
        && run.seen_working == seen_working
        && run.idle_since == idle_since
        && run.idle_no_report == idle_no_report
        && run.herdr_idle_since == herdr_idle_since
        && run.stuck == stuck
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
        if !crate::session_scope::owns_task(ctx, t)? {
            return Ok(false);
        }
        if t.status != from {
            return Ok(false);
        }
        let Some(r) = t
            .current_run_mut()
            .filter(|r| r.id == run_id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        if r.stuck != run.stuck
            || r.pending_reply != run.pending_reply
            || r.obligations != run.obligations
            || r.seen_pane != run.seen_pane
        {
            return Ok(false);
        }
        r.agent_state = Some(state.clone());
        r.seen_pane = seen_pane;
        r.seen_working = seen_working;
        r.idle_since = idle_since;
        r.idle_no_report = idle_no_report;
        r.herdr_idle_since = herdr_idle_since;
        r.stuck = stuck.clone();
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
    if newly_stuck && let Some(process) = &stuck {
        ctx.store.log_event(
            Some(&task.id), "stuck",
            json!({"run": run_id, "pid": process.pid, "argv": process.argv, "started": process.started}),
        )?;
        let note = format!("stuck: {}", process.argv.join(" "));
        if let Err(err) = ctx.herdr.notify(&format!("{} stuck", task.id), &note, true) {
            crate::messages::report(
                Some(&task.id),
                "notification",
                format!("stuck notification for {} failed: {err:#}", task.id),
            );
        }
        return Ok(Some(Change {
            task: task.id.clone(),
            from,
            to,
            note,
            stage: None,
        }));
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

fn start_queued(ctx: &Ctx, now: Timestamp) -> Result<Vec<Change>> {
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    let mut queued: Vec<Task> = ctx
        .store
        .list()?
        .into_iter()
        .filter(|t| {
            !t.tracker && (t.launch.is_some() || crate::queue::authorized_ready(&ctx.store, t))
        })
        .collect();
    crate::queue::sort_candidates(&mut queued);
    let mut changes = Vec::new();
    for (index, task) in queued.into_iter().enumerate() {
        let result = (|| {
            if !task.queued && task.launch.is_none() {
                crate::queue::enqueue_ready(ctx, &task.id, &DispatchOpts::default())?;
            }
            crate::dispatch::dispatch_queued(ctx, &task.id, index + 1, now)
        })();
        match result {
            Ok(DispatchOutcome::Started { run_id, .. }) => changes.push(Change {
                task: task.id.clone(),
                from: task.status,
                to: Status::Running,
                note: format!("dispatched queued task as {run_id}"),
                stage: None,
            }),
            Ok(DispatchOutcome::Queued { .. }) => {}
            Err(err) => {
                crate::messages::report(
                    Some(&task.id),
                    "dispatch",
                    format!("dispatching queued {} failed: {err:#}", task.id),
                );
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
