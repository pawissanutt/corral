use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use corral_core::{AutoStage, RunOutcome, Status, TaskHead};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use crate::{Ctx, ProcessControl, SystemProcesses};

#[path = "sync_activity.rs"]
mod activity;
use activity::omo_recently_active;

#[path = "sync_excerpt.rs"]
mod excerpt;
pub(crate) mod gate_watchdog;
use excerpt::read_excerpt;

#[cfg(test)]
#[path = "sync_held_tests.rs"]
mod held_tests;

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

/// Sync with the final pass-owned attention projection, without a second classification.
/// None means no owning pass obtained the attention reconciliation lease.
pub fn sync_with_attention(
    ctx: &Ctx,
) -> Result<(Vec<Change>, Option<crate::attention::AttentionBoard>)> {
    let mut attention = None;
    let changes = sync_passes(
        ctx,
        corral_core::clock::now(),
        &SystemProcesses,
        None,
        Some(&mut attention),
        &SyncScope::All,
    )?;
    Ok((changes, attention))
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
            None,
            &SyncScope::All,
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
    sync_passes(ctx, now, processes, None, None, &SyncScope::All)
}

/// One pass whose task effects reach only `scope`'s tasks; see [`SyncScope`].
pub(crate) fn sync_in(ctx: &Ctx, scope: &SyncScope) -> Result<Vec<Change>> {
    sync_passes(
        ctx,
        corral_core::clock::now(),
        &SystemProcesses,
        None,
        None,
        scope,
    )
}

fn sync_passes(
    ctx: &Ctx,
    now: Timestamp,
    processes: &dyn ProcessControl,
    source: Option<&Ctx>,
    attention: Option<&mut Option<crate::attention::AttentionBoard>>,
    scope: &SyncScope,
) -> Result<Vec<Change>> {
    // Latch replacement/schema skew before any preparation reopens a snapshot and
    // reports only a generic incompatible-schema error (T-587).
    ctx.store.check_runtime()?;
    if ctx.store.loop_owned_elsewhere()? {
        return Ok(Vec::new());
    }
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(Vec::new());
    }
    // Unobserved opens defer PM upgrades until this ownership check has succeeded.
    corral_core::agent::pm_state::migrate(&ctx.store)?;
    // Recovery and derivation preparation run before the measured passes.
    crate::settling::recover(ctx)?;
    let mut rebuild = false;
    let changes = hot_passes(ctx, now, processes, source, &mut rebuild, attention, scope)?;
    // Rebuildable state recovers outside the measured hot path (plan T-253 section 4).
    if rebuild && let Err(error) = crate::content_refresh::refresh_search_index(ctx) {
        crate::content_refresh::defer_corrupt_blob_cache_refresh(error)?;
    }
    Ok(changes)
}

/// The measured passes; `rebuild` is set when a pass could not follow the search index.
fn hot_passes(
    ctx: &Ctx,
    now: Timestamp,
    processes: &dyn ProcessControl,
    source: Option<&Ctx>,
    rebuild: &mut bool,
    mut attention: Option<&mut Option<crate::attention::AttentionBoard>>,
    scope: &SyncScope,
) -> Result<Vec<Change>> {
    let _hot = corral_core::work::HotScope::enter();
    let mut changes = Vec::new();
    let mut attempted = std::collections::BTreeSet::new();
    loop {
        // The change-feed position, never a serialization of board history.
        let before = ctx.store.view()?.seq();
        let pass = sync_scoped(
            ctx,
            now,
            processes,
            source,
            rebuild,
            attention.as_deref_mut(),
            scope,
        )?;
        let progressed = pass.iter().any(|change| change.to != Status::NeedsMe);
        changes.extend(pass);
        let after = ctx.store.view()?.seq();
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
                let task = ctx.store.head(id)?;
                // A scoped pass cannot act on a foreign action, so it cannot continue on one.
                if !scope.includes(task.project.as_deref()) {
                    continue;
                }
                // Terminal samples cannot create another attempt. Only a new
                // action, candidate/receipt, plan, round or policy can continue.
                let key = crate::verification::digest(&serde_json::to_vec(&(
                    &workset.policy_revision,
                    id,
                    action.authority,
                    &action.action.evidence,
                    task.rounds,
                    task.current_run
                        .as_ref()
                        .map(|run| (&run.id, &run.gate, &run.review)),
                    task.last_plan
                        .as_ref()
                        .map(|plan| (plan, plan.facts.children_rev)),
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
    rebuild: &mut bool,
    attention: Option<&mut Option<crate::attention::AttentionBoard>>,
    scope: &SyncScope,
) -> Result<Vec<Change>> {
    // Reuse short ownership views, but re-read an expired guard after a slow external
    // step. This never retries the step; inventory still re-reads under its lease.
    let mut fresh = ctx.clone();
    fresh.store.begin_refreshable_session_view()?;
    if !crate::session_scope::owns_inventory(&fresh)? {
        return Ok(Vec::new());
    }
    fresh.reload_config()?;
    let ctx = &fresh;
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(Vec::new());
    }
    ctx.store.check_runtime()?;
    crate::hold::reconcile(ctx, scope)?;
    crate::desktop_notifications::reconcile(ctx)?;
    crate::report::scheduled_digest_and_report_at(ctx, now)?;
    // Status moves on any project's legacy queue entries and trackers: workspace sync only.
    let mut changes = scope.workspace_only(|| crate::queue::migrate_backlog(ctx))?;
    changes.extend(scope.workspace_only(|| crate::tracker::reconcile(ctx))?);
    let resources = crate::resources::observe(ctx, now)?;
    // A holding observation already fences this operation. Reuse its real admission for
    // attention; launch-capable paths still perform their own fresh admission checks.
    let held_admission = resources
        .reason
        .as_ref()
        .filter(|_| resources.held)
        .map(|reason| format!("waiting: machine busy ({reason})"));
    changes.extend(resource_phase(ctx, &resources, now)?);
    crate::lesson_distill::announce_due(ctx, now)?;
    let herdr_available = ctx.herdr.available();
    if let Err(error) = crate::pm_policy::reconcile_pm_policy(ctx, herdr_available) {
        // Policy-delivery trouble is visible, but cannot stop receipt collection
        // or unrelated already-authorized work.
        crate::messages::report(None, "pm-policy", format!("{error:#}"));
    }
    if !herdr_available {
        scope.workspace_only(|| crate::hooks::reconcile_post_land(ctx))?;
        crate::gate::collect_finished(ctx, now)?;
        crate::storage_gc::lifecycle_pass(ctx, now)?;
        let search_stamp = crate::content_refresh::task_stamp(ctx)?;
        for head in scope.heads(open_heads(ctx)?) {
            crate::obligations_sync::reconcile_head(ctx, &ctx.store.head(&head.id)?, now, None)?;
        }
        scope.workspace_only(|| crate::movement::reconcile(ctx, now, false))?;
        scope.workspace_only(|| {
            reconcile_attention(ctx, now, None, attention, held_admission.as_deref())
        })?;
        crate::pm_events::push_events(ctx, false)?;
        crate::desktop_notifications::reconcile(ctx)?;
        match crate::content_refresh::refresh_after_sync_if_due(ctx, &[], &search_stamp) {
            Ok(followed) => *rebuild |= !followed,
            Err(error) => crate::content_refresh::defer_corrupt_blob_cache_refresh(error)?,
        }
        return Ok(changes);
    }
    // Wait out GC's exclusive storage before taking the reconciliation lease every finishing
    // launch needs, so a write in this pass never parks it (T-505). The pass holds no storage:
    // each artifact write shares it only around itself (T-659).
    crate::artifact_storage::await_storage(ctx)?;
    let Some(inventory) = crate::pane_liveness::inventory(ctx) else {
        // Inventory contention/UNKNOWN must stop pane decisions and new launches,
        // not authenticated completion receipts. Use a receipt-only ownership
        // lease here; the healthy path already owns one through inventory.
        if let Some(_ownership) = ctx.store.try_begin_session_pass()? {
            let board = ctx.store.board(None)?.detach();
            let source_owned = source
                .map(|source| crate::session_scope::owns_board(source, &ctx.store, &board))
                .transpose()?
                .unwrap_or(true);
            if source_owned && crate::session_scope::owns_board(ctx, &ctx.store, &board)? {
                crate::gate::collect_finished(ctx, now)?;
            }
        }
        scope.workspace_only(|| {
            reconcile_attention(ctx, now, None, attention, held_admission.as_deref())
        })?;
        return Ok(changes);
    };
    let search_stamp = crate::content_refresh::task_stamp(ctx)?;
    let board = ctx.store.board(None)?.detach();
    crate::agent::effects::failpoint("sync_tasks_read");
    if !crate::session_scope::owns_board(ctx, &ctx.store, &board)? {
        return Ok(changes);
    }
    if let Some(source) = source
        && !crate::session_scope::owns_board(source, &ctx.store, &board)?
    {
        return Ok(changes);
    }
    crate::gate::collect_finished(ctx, now)?;
    crate::scratch_reclaim::reclaim_scratch_pass(ctx, now)?;
    crate::storage_gc::lifecycle_pass(ctx, now)?;
    let panes: HashMap<String, PaneInfo> = inventory
        .panes
        .iter()
        .cloned()
        .map(|p| (p.pane_id.clone(), p))
        .collect();
    let snapshot = inventory.snapshot(&panes);
    crate::cleanup::sweep_inventory(ctx, &board, &inventory.panes);
    // Each task's head is read afresh: gate collection above may have just recorded a result.
    for head in board
        .heads
        .iter()
        .filter(|head| scope.includes(head.project.as_deref()))
    {
        if let Some(change) =
            crate::obligations_sync::reconcile_one(ctx, &head.id, now, Some(&panes))?
        {
            changes.push(change);
        }
    }
    let mut native: BTreeMap<_, _> = ctx
        .store
        .unfinished_native_heads()?
        .into_iter()
        .map(|head| (head.id.clone(), head.detach()))
        .collect();
    for head in scope.heads(open_heads(ctx)?) {
        if !crate::session_scope::owns_head(ctx, &head)? {
            continue;
        }
        crate::plan_pilot::reconcile(ctx, &head.to_shell_task()?, snapshot, now)?;
        let reconciled = ctx.store.head(&head.id)?;
        if reconciled.status != head.status {
            changes.push(Change {
                task: head.id.clone(),
                from: head.status,
                to: reconciled.status,
                note: reconciled
                    .last_plan_run
                    .as_ref()
                    .and_then(|r| r.pending_reply.clone())
                    .unwrap_or_else(|| "planner agent state changed".into()),
                stage: None,
            });
        }
        let current = reconciled.to_shell_task()?;
        if let Some(change) = crate::launch_exit::plan_review(ctx, &current, snapshot, now)? {
            changes.push(change);
        }
        crate::plan_acceptance::reconcile_exit(ctx, &current)?;
        if let Some(change) =
            crate::reviewer::reconcile_review_sampled_at(ctx, &head.id, now, snapshot)?
        {
            changes.push(change);
        }
        // One hook call per task holding live corral-pi work; other tasks never reach it.
        let (head, agent) = if native.remove(&head.id).is_some() {
            let synced = crate::agent::hooks::sync::task_id(ctx, &head.id, now)?;
            (ctx.store.head(&head.id)?, synced)
        } else {
            (ctx.store.head(&head.id)?, Default::default())
        };
        let shell = head.to_shell_task()?;
        let agent_healthy = agent.current_worker_healthy(&shell);
        close_held_run(ctx, &head, &panes, now)?;
        if !crate::plan_pilot::handles(&shell) && head.status.holds_slot() && head.launch.is_none()
        {
            if let Some(change) = crate::provider_stall::advance_agent(ctx, &head, &panes, now)? {
                changes.push(change);
                continue;
            }
            if let Some(change) = reconcile(
                ctx,
                &head,
                snapshot,
                now,
                processes,
                agent_healthy,
                agent.attention.as_deref(),
            )? {
                changes.push(change);
            }
        }
    }
    // Indexed owed work survives terminal status and retired planning/review containers.
    for (_, head) in native {
        if scope.includes(head.project.as_deref()) && crate::session_scope::owns_head(ctx, &head)? {
            crate::agent::hooks::sync::task_id(ctx, &head.id, now)?;
        }
    }
    scope.workspace_only(|| crate::delivery::owner::reconcile_all(ctx))?;
    crate::pm_inbox::sweep(ctx)?;
    crate::agent::hooks::sync::pass(ctx)?;
    crate::review_budget::reconcile(ctx, snapshot, now)?;
    // Launching queued/autopilot work must acquire an exclusive ownership claim.
    // No consumer below this point receives the sampled pane IDs.
    drop(inventory);
    // Post-land hooks run commands that may call corral: never under storage or the lease (T-509).
    scope.workspace_only(|| crate::hooks::reconcile_post_land(ctx))?;
    scope.workspace_only(|| crate::assistant::workflow::handoff::recovery::reconcile(ctx))?;
    scope.workspace_only(|| crate::hold::resume_gates(ctx))?;
    let native = scope.heads(ctx.store.unfinished_native_heads()?);
    crate::agent::gate_queue::advance_heads(ctx, &native)?;
    crate::plan_pilot::advance_all(ctx, now, scope)?;
    changes.extend(scope.workspace_only(|| crate::availability::advance_all(ctx, now))?);
    changes.extend(crate::autopilot::advance_all(ctx, now, scope)?);
    crate::hold::reconcile(ctx, scope)?;
    scope.workspace_only(|| crate::movement::reconcile(ctx, now, true))?;
    changes.extend(scope.workspace_only(|| crate::dashboard::advance(ctx, now))?);
    changes.extend(crate::parked::reacquire_woken(
        ctx,
        &scope.heads(open_heads(ctx)?),
    )?);
    changes.extend(crate::queue_drain::start_queued(ctx, now, scope)?);
    changes.extend(crate::dispatch_policy::advance(ctx, now, scope)?);
    scope.workspace_only(|| {
        let pm_live = Some(crate::pm::live_pm_pane(ctx)?.is_some());
        reconcile_attention(ctx, now, pm_live, attention, held_admission.as_deref())
    })?;
    crate::agent::effects::failpoint("sync_events");
    crate::pm_events::push_events(ctx, true)?;
    crate::desktop_notifications::reconcile(ctx)?;
    match crate::content_refresh::refresh_after_sync_if_due(ctx, &[], &search_stamp) {
        Ok(followed) => *rebuild |= !followed,
        Err(error) => crate::content_refresh::defer_corrupt_blob_cache_refresh(error)?,
    }
    // External container IPC follows admission and holds no inventory lease.
    if let Err(error) = crate::sandbox_cleanup::sweep(ctx) {
        crate::messages::report(None, "sandbox-cleanup", format!("{error:#}"));
    }
    Ok(changes)
}

#[path = "sync/read_phase.rs"]
mod read_phase;
use read_phase::{open_heads, watchdog_inputs};

#[path = "sync/held_phase.rs"]
mod held_phase;
use held_phase::close_held_run;

#[path = "sync/scope.rs"]
mod scope;
pub(crate) use scope::SyncScope;

#[path = "sync/resource_phase.rs"]
mod resource_driver;
pub(crate) use resource_driver::reconcile as resource_phase;

#[path = "sync/attention_phase.rs"]
mod attention_driver;
use attention_driver::reconcile as reconcile_attention;

fn reconcile(
    ctx: &Ctx,
    head: &TaskHead,
    snapshot: crate::pane_liveness::Snapshot<'_>,
    now: Timestamp,
    processes: &dyn ProcessControl,
    agent_healthy: bool,
    agent_attention: Option<&str>,
) -> Result<Option<Change>> {
    let panes = snapshot.panes;
    let _attribution = ctx.store.attribute(corral_core::Actor::System);
    if !head.status.holds_slot() {
        return Ok(None);
    }
    let shell = head.to_shell_task()?;
    let task = &shell;
    if crate::resource_escalation::holds_attention(ctx, task)? {
        return Ok(None);
    }
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    // The shell run carries no obligations: their current state is the run's facts.
    let Some(facts) = head.current_run.as_ref().map(|r| &r.facts) else {
        return Ok(None);
    };
    let Some(pane_id) = run.herdr.as_ref().map(|h| h.pane_id.clone()) else {
        return Ok(None);
    };
    // Detach keyed inputs before herdr/process work, never after its unbounded wait.
    let Some(watchdog_run) = watchdog_inputs(ctx, head, run)? else {
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
            task: head.into(),
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
    let exited = !working && crate::launch_exit::exited_for(ctx, &task.id, &run.id);
    let state = match pane {
        None if !gone => run.agent_state.clone().unwrap_or_else(|| "starting".into()),
        None => "gone".to_string(),
        Some(_) if exited => "exited".to_string(),
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
    // A parked worker is deliberately idle, not blocked by its foreground launcher.
    // Do not suppress a slot_wait alone: expired/withdrawn waits must expose stuck work again.
    let parked = facts.owner_task == task.id
        && facts.parked_on_dependencies()
        && !run
            .gate
            .as_ref()
            .is_some_and(corral_core::GateRecord::holds_slot)
        && !run
            .review
            .as_ref()
            .is_some_and(corral_core::ReviewRecord::holds_slot);
    let stuck =
        if !parked
            && !gate_watchdog::pending(head, run)?
            && herdr_idle_since.is_some_and(|since| now.duration_since(since) > stuck_after)
        {
            match ctx.herdr.pane_process_info(&pane_id).and_then(|info| {
                crate::watchdog::foreign_process(ctx, &watchdog_run, &info, processes)
            }) {
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
    let decision = facts.pending_decision.as_ref();
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
    let waiting = facts.pending_waits > 0;
    let obligations_rev = facts.obligations_rev;
    // A corral-pi question that waits on the owner holds Needs me like an owner decision.
    let (to, note) =
        if decision.is_some() || harness_question.is_some() || agent_attention.is_some() {
            (Status::NeedsMe, "owner decision pending")
        } else if exited {
            (
                Status::NeedsMe,
                "agent exited to its shell before reporting",
            )
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
    // A conversation reopened for changes can answer with its old "complete" state and idle,
    // whether or not an earlier sync happened to sample it working; once startup is over, that
    // idle turn is a report gap at the first idle sync, not after `idle_after_secs`.
    let startup_grace = SignedDuration::from_secs(
        i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap_or(i64::MAX),
    );
    let silent_round = run
        .changes_round
        .as_ref()
        .is_some_and(|round| now.duration_since(round.at) >= startup_grace);
    // A healthy corral-pi binding classifies its turns causally; idleness is not a report gap.
    let (idle_since, idle_no_report) =
        if to != Status::Running || working || waiting || agent_healthy {
            (None, false)
        } else if looks_idle && (seen_working || silent_round) {
            let since = run.idle_since.unwrap_or(now);
            let flagged =
                run.idle_no_report || silent_round || now.duration_since(since) >= idle_after;
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
    } else if let Some(attention) = agent_attention {
        Some(attention.to_owned())
    } else if pane.is_none() {
        Some("agent pane is gone".to_string())
    } else {
        Some(read_excerpt(ctx, &pane_id))
    };
    // While still waiting, keep the excerpt current: an agent may print its question just after
    // herdr flags it blocked, so the snapshot taken on entry can miss it.
    let refreshed = if to == Status::NeedsMe
        && !entering_needs_me
        && pane.is_some()
        && decision.is_none()
        && harness_question.is_none()
        && agent_attention.is_none()
        && !stall_escalated
    {
        Some(read_excerpt(ctx, &pane_id))
            .filter(|e| !e.is_empty() && run.blocked_excerpt.as_ref() != Some(e))
    } else {
        None
    };
    let excerpt = excerpt.or(refreshed).map(excerpt::launch_refusal_hint);

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
    let exit_reason = exited.then(|| {
        let capture = excerpt
            .clone()
            .unwrap_or_else(|| read_excerpt(ctx, &pane_id));
        format!("The agent exited to its shell before reporting.\n{capture}")
    });
    let mut notices = Vec::new();
    // Everything above was decided from the snapshot `sync` listed; herdr calls since then leave time
    // for a CLI writer (`corral done`, move, retry, ...). Apply only if the task is still in the state
    // we reconciled: same status, same run, run still open. Otherwise skip; the next tick re-reads.
    let written = ctx.store.try_update_head(&task.id, |t| {
        if !crate::session_scope::owns_head(ctx, t.head())? {
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
        // obligations_rev stands in for the whole obligations: stricter only on A->B->A.
        if r.stuck != run.stuck
            || r.pending_reply != run.pending_reply
            || r.facts.obligations_rev != obligations_rev
            || r.obligations.slot_wait != run.obligations.slot_wait
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
        if exited {
            // The agent is gone: end the attempt so Needs me holds no dispatch slot.
            r.outcome = Some(RunOutcome::Failed);
            r.ended = Some(now);
        }
        t.status = to;
        if let Some(reason) = &exit_reason {
            ctx.store.log_event(
                Some(&task.id),
                "run_failed",
                json!({"run": run_id, "reason": reason}),
            )?;
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
        } else if to != from {
            let kind = if to == Status::NeedsMe {
                "needs_me"
            } else {
                "resumed"
            };
            ctx.store.log_event(
                Some(&task.id),
                kind,
                json!({ "from": from.as_str(), "agent_state": state, "note": note,
                    "question_keys": if note == "agent is blocked" {
                        crate::questions::pending_keys_ref(t)?
                    } else { Vec::new() } }),
            )?;
        }
        if entering_needs_me {
            notices.push(crate::owner_notice::record_entry_head(
                ctx,
                t,
                "needs_me_entry",
                now,
                Vec::new(),
            )?);
        }
        if newly_stuck && let Some(process) = &stuck {
            let source = format!("{run_id}:{}:{}", process.pid, process.started);
            let claim = crate::owner_notice::prepare(
                ctx,
                t,
                "stuck",
                &source,
                now,
                vec![corral_core::NoticeCoverage::Exact {
                    condition_key: format!("{}/stuck_process/{source}", t.id),
                }],
            )?;
            let pending = match notices.first() {
                Some(first) => first.sibling(claim.batch_id.clone()),
                None => crate::owner_notice::reserve_task(ctx, &t.id, claim.batch_id.clone())?,
            };
            notices.push(pending);
            t.attention.push_notice(claim)?;
        }
        Ok(true)
    })?;
    let Some(written) = written else {
        return Ok(None);
    };
    for batch in notices {
        let (title, body) = if written
            .attention
            .notice_outbox
            .iter()
            .any(|claim| claim.batch_id == batch.batch_id && claim.producer == "stuck")
        {
            let process = stuck.as_ref().context("stuck notice has no process")?;
            (
                format!("{} stuck", task.id),
                format!("stuck: {}", process.argv.join(" ")),
            )
        } else {
            (
                format!("▲ {} blocked", task.id),
                excerpt
                    .as_deref()
                    .unwrap_or(note)
                    .lines()
                    .last()
                    .unwrap_or(note)
                    .to_owned(),
            )
        };
        crate::owner_notice::notify(
            ctx,
            crate::owner_notice::newly_committed(&written, batch)?,
            &title,
            &body,
            true,
        )?;
    }
    if let Some(reason) = &exit_reason {
        crate::launch_exit::record(ctx, &task.id, &run_id, "Agent launch failure", reason)?;
    }
    if newly_stuck && let Some(process) = &stuck {
        let note = format!("stuck: {}", process.argv.join(" "));
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
    Ok(Some(Change {
        task: task.id.clone(),
        from,
        to,
        note: note.to_string(),
        stage: None,
    }))
}

#[cfg(test)]
#[path = "agent/hooks/sync_seam_tests.rs"]
mod agent_seam_tests;

#[cfg(test)]
#[path = "sync_guard_tests.rs"]
mod guard_tests;

#[cfg(test)]
#[path = "sync_lease_tests.rs"]
mod lease_tests;
