//! Sync seam: one hook call per task that holds corral-pi work, and one per open container
//! inside it. It persists F4 question terminalization, owner deadlines, the unattended rule,
//! gate reservations and uncertain gate starts, due wakes and run-end cleanup. It reports each
//! open container's binding health, so sync leaves `idle_no_report` unset for a healthy host,
//! and any pending question that waits on the owner, so sync's status decision keeps that
//! attention. A closed container is settled once and then never revisited, its Pi sessions
//! reclassified as a terminal transcript. Each open container's healthy host is probed: a
//! verified loss is recorded `agent_lost` and holds a live worker in Needs me with resume,
//! retry and discard (an open reviewer whose pane stays, with resume), and a call a recovery notice reported `in_progress` gets its one
//! `recovery_update` once it settles. Owner: T-252 children worker-role (wakes, turns) and
//! recovery (host loss).
use std::time::Duration;

use anyhow::{Context, Result};
use corral_core::agent::AgentRunState;
use corral_core::agent::{
    ContainerRef, HostHealth, ReceiptState, TurnClass, container_closure, container_state, paths,
    pm_state,
};
use corral_core::inbox::Recipient;

use super::super::bridge::liveness::{LossReason, Probe};
use corral_core::{Run, Status, Task, TaskRef};
use jiff::Timestamp;

use super::{consulted, run_is_corral_pi};
use crate::Ctx;

/// The watchdog needs the current native generation, which compatibility shells omit.
/// Read only that keyed row on the head's view, never the accumulated generation tail.
pub(crate) fn watchdog_run(head: &corral_core::TaskHead, run: &Run) -> Result<Run> {
    let mut projected = run.clone();
    if let Some(host) = projected.agent_host.as_mut() {
        let container = ContainerRef::Worker {
            task: head.id.clone(),
            run: run.id.clone(),
        };
        host.generations = head
            .agent_generation(&container, host.binding.generation)?
            .into_iter()
            .collect();
    }
    Ok(projected)
}

/// One open corral-pi container and whether its host binding is healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSync {
    pub container: ContainerRef,
    pub healthy: bool,
}

/// What one hook call reports back to sync's per-task status decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskSync {
    pub open: Vec<ContainerSync>,
    /// A pending question that waits on the owner (escalated or marked unattended), or a worker
    /// turn that ended without a report after its generation's nudge: sync keeps the task in
    /// Needs me until the question settles or another turn starts.
    pub attention: Option<String>,
}

impl TaskSync {
    /// Whether the task's current worker run is open with a healthy binding.
    pub fn current_worker_healthy(&self, task: &Task) -> bool {
        task.current_run().is_some_and(|run| {
            self.open.iter().any(|c| {
                c.healthy
                    && matches!(&c.container, ContainerRef::Worker { run: id, .. } if *id == run.id)
            })
        })
    }
}

/// Every corral-pi worker run and review child of the task, open or closed.
fn containers(task: &Task) -> Vec<(ContainerRef, &Run)> {
    let mut out = Vec::new();
    for run in &task.runs {
        if run_is_corral_pi(run) {
            let worker = ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            };
            out.push((worker, run));
        }
        for child in run.review_runs.iter().filter(|c| run_is_corral_pi(c)) {
            let reviewer = ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            };
            out.push((reviewer, child));
        }
    }
    out
}

/// A closed container still owes run-end duties: it was never settled (its messages and
/// runtime), or a request reservation of it is still unsettled.
fn owes(task: &Task, container: &ContainerRef, run: &Run) -> bool {
    run.inbox.settled.is_none()
        || container_state(task, container).is_some_and(|state| {
            state.questions.iter().any(|q| q.recorded_pending())
                || state.jobs.iter().any(|job| job.needs_reap())
                || state
                    .receipts
                    .iter()
                    .any(|r| r.effects_pending() || r.recovery_update_due())
        })
}

/// Pure and cheap: whether sync calls `task` for this task at all. A settled closed container
/// never makes it true again.
pub fn wants(task: &Task) -> bool {
    containers(task).iter().any(|(container, run)| {
        container_closure(task, container).is_none() || owes(task, container, run)
    }) || super::super::ops::planning::wants_sync(task)
        || task.agent_pm_receipts.iter().any(|receipt| {
            matches!(
                receipt.state,
                ReceiptState::Reserved | ReceiptState::Queued | ReceiptState::Uncertain
            ) || !receipt.post_pending.is_empty()
        })
}

pub fn task(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<TaskSync> {
    task_id(ctx, &task.id, now)
}

/// Indexed sync callers already hold the task identity, not a manufactured Task facade.
pub(crate) fn task_id(ctx: &Ctx, id: &str, now: Timestamp) -> Result<TaskSync> {
    super::super::effects::reconcile_task_hot(ctx, id)?;
    crate::questions::terminalize(ctx, id)?;
    crate::questions::expire_deadlines(ctx, id, now)?;
    crate::questions::unattended(ctx, id, now)?;
    crate::questions::hold_owner_attention(ctx, id)?;
    super::super::turns::nudge(ctx, id)?;
    super::super::turns::settle_hold(ctx, id)?;
    probe_hosts(ctx, &ctx.store.head(id)?)?;
    super::super::recovery::post_updates(ctx, id)?;
    settle_lost(ctx, id)?;
    super::super::jobs::reap(ctx, id, now)?;
    super::review::hold_lost(ctx, id)?;
    super::review::release_lost(ctx, id)?;
    let current = ctx.store.head(id)?;
    let uncertain = Duration::from_secs(ctx.store.config.agent.gate_start_uncertain_secs);
    let mut due = crate::review_budget::due_gate_wakes_ref(&current, now, uncertain)?;
    due.extend(super::super::wake::due_wait_wakes_ref(&current)?);
    super::super::wake::post_due(ctx, &due)?;
    super::cleanup::run_end_ref(ctx, &current)?;
    settle(ctx, &current, now)?;
    let mut synced = TaskSync {
        attention: attention_ref(&current)?,
        ..TaskSync::default()
    };
    for container in current.unfinished_native_containers()? {
        if !matches!(
            container,
            ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. }
        ) || corral_core::agent::container_closure_ref(&current, &container)?.is_some()
        {
            continue;
        }
        consulted("sync");
        let healthy = current
            .agent_head(&container)?
            .is_some_and(|host| host.binding.health == HostHealth::Healthy);
        if synced.attention.is_none() {
            synced.attention = current
                .pending_agent_questions(&container)?
                .into_iter()
                .find(|question| {
                    question.recorded_pending() && crate::questions::needs_owner(question)
                })
                .map(|question| {
                    format!(
                        "corral-pi question {} needs you: {}",
                        question.key, question.text
                    )
                });
        }
        synced.open.push(ContainerSync { container, healthy });
    }
    if synced.attention.is_none() {
        synced.attention = super::super::turns::attention_ref(&current)?;
    }
    Ok(synced)
}

/// Records each newly closed container as settled once terminalization cancelled its
/// undelivered Pi messages, run-end cleanup removed its runtime above, and owner delivery has
/// nothing left to retire or finish for it (otherwise the next pass settles it).
fn settle<T: TaskRef + ?Sized>(ctx: &Ctx, task: &T, now: Timestamp) -> Result<()> {
    let mut closed = Vec::new();
    for container in task.unfinished_native_containers()? {
        if !matches!(
            container,
            ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. }
        ) || corral_core::agent::container_closure_ref(task, &container)?.is_none()
        {
            continue;
        }
        let Some(recipient) = crate::questions::recipient(&container) else {
            continue;
        };
        if task
            .inbox_head(&recipient)?
            .is_none_or(|inbox| inbox.settled.is_some())
        {
            continue;
        }
        // Late evidence for an orphaned attempt may never come once the host is gone.
        let work = crate::delivery::owner::work(ctx, &recipient)?;
        if !work.unsettled && !work.effects {
            closed.push((container, recipient));
        }
    }
    if closed.is_empty() {
        return Ok(());
    }
    let mut settled = Vec::new();
    ctx.store.try_update_head(task.id(), |current| {
        settled.clear();
        for (container, _) in &closed {
            if current.settle_native_inbox(container, now)? {
                settled.push(container.clone());
            }
        }
        Ok(!settled.is_empty())
    })?;
    for container in settled {
        reclassify(ctx, task, &container)?;
    }
    Ok(())
}

/// A settled container's Pi sessions stop being recovery-critical: they stay where they are as
/// its terminal transcript, recorded once by `agent_session_reclassified`.
fn reclassify<T: TaskRef + ?Sized>(ctx: &Ctx, task: &T, container: &ContainerRef) -> Result<()> {
    let dir = paths::container_sessions_dir(&ctx.store, container);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut sessions: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".jsonl"))
        .collect();
    sessions.sort();
    let closure =
        corral_core::agent::container_closure_ref(task, container)?.map(|reason| reason.as_str());
    ctx.store.log_event(
        container.task(),
        "agent_session_reclassified",
        serde_json::json!({
            "container": container.key(), "class": "terminal_transcript", "dir": dir,
            "sessions": sessions, "closure": closure,
        }),
    )?;
    Ok(())
}

/// Probes every open container's healthy host and records a verified loss or the reconciled
/// event sequence. A failed probe is reported and retried next pass, never fatal to sync.
fn probe_hosts<T: TaskRef + ?Sized>(ctx: &Ctx, task: &T) -> Result<()> {
    for container in task.unfinished_native_containers()? {
        if corral_core::agent::container_closure_ref(task, &container)?.is_some() {
            continue;
        }
        let Some(host) = task.agent_head(&container)? else {
            continue;
        };
        let Some(recipient) = crate::questions::recipient(&container) else {
            continue;
        };
        if let Err(error) = consume_exit(ctx, task, &container) {
            let reason = format!("reading the host exit of {}: {error:#}", container.key());
            crate::messages::report(container.task(), "agent liveness", reason);
        }
        if host.binding.health != HostHealth::Healthy || !super::super::bridge::liveness::probing()
        {
            continue;
        }
        consulted("sync");
        let probed =
            super::super::bridge::liveness::probe_ref(&ctx.store, task, &recipient, &container)
                .and_then(|probe| {
                    let recorded =
                        super::super::bridge::liveness::record(&ctx.store, &container, &probe)?;
                    if recorded
                        && let Probe::Lost { generation, .. } = probe
                        && let Some(lost) = task.agent_generation(&container, generation)?
                    {
                        super::super::herdr_status::release_after_loss(ctx, &container, &lost);
                    }
                    Ok(recorded)
                });
        if let Err(error) = probed {
            crate::messages::report(
                container.task(),
                "agent liveness",
                format!("probing {} failed: {error:#}", container.key()),
            );
        }
    }
    Ok(())
}

/// A worker or reviewer host that exits leaves its pane wrapper's launch-exit receipt. For a
/// generation that completed hello that is a host loss, resumable, not a launch failure: the
/// receipt is consumed into `agent_lost`, so neither sync's launch-exit rule nor the review
/// slot's reviewer-exit rule ends the run or closes the review child. A generation that exited
/// before hello keeps its receipt and fails at launch as before.
fn consume_exit<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    source: &T,
    container: &ContainerRef,
) -> Result<()> {
    let (task, attempt) = match container {
        ContainerRef::Worker { task, run } => (task, run.clone()),
        ContainerRef::Reviewer { task, run, review } => (task, format!("{run}-{review}")),
        _ => return Ok(()),
    };
    let Some(host) = source.agent_head(container)? else {
        return Ok(());
    };
    let Some(generation) = source
        .agent_generation(container, host.binding.generation)?
        .filter(|generation| generation.bound.is_some())
    else {
        return Ok(());
    };
    let receipt = crate::launch_exit::receipt(ctx, task, &attempt)?;
    let status = match std::fs::read_to_string(&receipt) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", receipt.display())),
    };
    let loss = Probe::Lost {
        generation: generation.n,
        reason: LossReason::HostExited,
        detail: format!(
            "the host exited to its pane shell with status {}",
            status.trim()
        ),
    };
    if super::super::bridge::liveness::record(&ctx.store, container, &loss)? {
        super::super::herdr_status::release_after_loss(ctx, container, &generation);
    }
    match std::fs::remove_file(&receipt) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("consuming {}", receipt.display())),
    }
}

fn probe_one(ctx: &Ctx, recipient: &Recipient, container: &ContainerRef, state: &AgentRunState) {
    if state.binding.health != HostHealth::Healthy || !super::super::bridge::liveness::probing() {
        return;
    }
    consulted("sync");
    let probed = super::super::bridge::liveness::probe(&ctx.store, recipient, container, state)
        .and_then(|probe| {
            let recorded = super::super::bridge::liveness::record(&ctx.store, container, &probe)?;
            Ok((recorded, probe))
        });
    match probed {
        Ok((true, Probe::Lost { generation, .. })) => {
            if let Some(lost) = state.generation(generation) {
                super::super::herdr_status::release_after_loss(ctx, container, lost);
            }
        }
        Ok(_) => {}
        Err(error) => crate::messages::report(
            container.task(),
            "agent liveness",
            format!("probing {} failed: {error:#}", container.key()),
        ),
    }
}

/// Once per sync pass: the PM wake outbox and the PM's host, while a corral-pi PM record exists,
/// and the host of every open assistant session.
pub fn pass(ctx: &Ctx) -> Result<()> {
    if pm_state::load_current(&ctx.store)?.is_some() {
        consulted("sync");
        super::super::pm_wake::repost(ctx)?;
        super::super::pm_wait_wake::sweep(ctx)?;
        if let Some(pm) = pm_state::load_current(&ctx.store)? {
            probe_one(ctx, &Recipient::Pm, &ContainerRef::Pm, &pm.state);
        }
    }
    for session in ctx.store.active_assistant_ids()? {
        let container = ContainerRef::Assistant {
            session: session.clone(),
        };
        match ctx.store.get_assistant_container(&session) {
            Ok(assistant) => {
                let recipient = Recipient::Assistant { session };
                probe_one(ctx, &recipient, &container, &assistant.state);
            }
            Err(error) => crate::messages::report(
                None,
                "agent liveness",
                format!("reading {}: {error:#}", container.key()),
            ),
        }
    }
    Ok(())
}

/// The Needs me excerpt prefixes of a live worker whose host is lost or whose provider
/// authentication failed; `settle_lost` releases them once the cause is gone.
pub const LOST_PREFIX: &str = "corral-pi host lost: ";
pub const AUTH_PREFIX: &str = "corral-pi provider authentication failed: ";

/// Why the task's live worker needs the owner: its host is lost (resume, retry or discard), or
/// its latest turn failed provider authentication (`corral agent login`, never a paid fallback).
pub fn attention(task: &Task) -> Option<String> {
    let run = task.current_run()?;
    let state = run.agent_host.as_deref()?;
    let container = ContainerRef::Worker {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    if container_closure(task, &container).is_some() {
        return None;
    }
    let n = state.binding.generation;
    if state.binding.health == HostHealth::Lost {
        return Some(format!(
            "{LOST_PREFIX}{} generation {n}. `corral agent resume {}` starts generation {} in \
             the same pane; `corral retry {1}` or `corral discard {1}` end the run instead.",
            container.key(),
            task.id,
            n + 1
        ));
    }
    let latest = state
        .turns
        .iter()
        .filter(|t| t.generation == n)
        .max_by_key(|t| t.seq)?;
    let failed = latest.class == Some(TurnClass::ProviderFailed) && latest.ended.is_some();
    (failed && latest.provider_error.as_deref() == Some("auth")).then(|| {
        format!(
            "{AUTH_PREFIX}turn {n}/{} of {}. Run `corral agent login`, then reply to the run to \
             continue; no paid fallback is used.",
            latest.seq,
            container.key()
        )
    })
}

/// The same worker cause from actual bounded metadata and its keyed latest turn.
pub(crate) fn attention_ref<T: TaskRef + ?Sized>(task: &T) -> Result<Option<String>> {
    let Some(run) = task.current_run_head()? else {
        return Ok(None);
    };
    let Some(host) = run.agent_host.as_deref() else {
        return Ok(None);
    };
    let container = ContainerRef::Worker {
        task: task.id().clone(),
        run: run.id.clone(),
    };
    if corral_core::agent::container_closure_ref(task, &container)?.is_some() {
        return Ok(None);
    }
    let n = host.binding.generation;
    if host.binding.health == HostHealth::Lost {
        return Ok(Some(format!(
            "{LOST_PREFIX}{} generation {n}. `corral agent resume {}` starts generation {} in \
             the same pane; `corral retry {1}` or `corral discard {1}` end the run instead.",
            container.key(),
            task.id(),
            n + 1
        )));
    }
    let latest = task.latest_agent_turn(&container, n)?;
    Ok(latest.filter(|turn| turn.class == Some(TurnClass::ProviderFailed)
        && turn.ended.is_some() && turn.provider_error.as_deref() == Some("auth"))
        .map(|turn| format!(
            "{AUTH_PREFIX}turn {n}/{} of {}. Run `corral agent login`, then reply to the run to \
             continue; no paid fallback is used.", turn.seq, container.key()
        )))
}

/// Returns a task this module held in Needs me to Running once its cause is gone: resume bound
/// another generation, or a later turn followed the failed authentication.
pub fn settle_lost(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let settled = ctx.store.try_update_head(task_id, |task| {
        if task.status != Status::NeedsMe || attention_ref(task)?.is_some() {
            return Ok(false);
        }
        let Some(run) = task.current_run_mut() else {
            return Ok(false);
        };
        let held = run.blocked_excerpt.as_deref().is_some_and(|e| {
            // Older binaries persisted these excerpts as machine-readable holds.
            [
                LOST_PREFIX,
                AUTH_PREFIX,
                "Pi-native host lost: ",
                "Pi-native provider authentication failed: ",
            ]
            .iter()
            .any(|prefix| e.starts_with(prefix))
        });
        if !held || run.facts.pending_decision.is_some() {
            return Ok(false);
        }
        run.blocked_excerpt = None;
        task.status = Status::Running;
        Ok(true)
    })?;
    Ok(settled.is_some())
}
