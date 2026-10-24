//! Sync seam: one hook call per task that holds pi-native work, and one per open container
//! inside it. It persists F4 question terminalization, owner deadlines, the unattended rule,
//! gate reservations and uncertain gate starts, due wakes and run-end cleanup. It reports each
//! open container's binding health, so sync leaves `idle_no_report` unset for a healthy host,
//! and any pending question that waits on the owner, so sync's status decision keeps that
//! attention. A closed container is settled once and then never revisited. Owner: T-252
//! children worker-role (wakes, turns) and recovery (host loss).
use std::time::Duration;

use anyhow::Result;
use corral_core::agent::{
    ContainerRef, HostHealth, ReceiptState, container_closure, container_state, paths,
};
use corral_core::{Run, Task};
use jiff::Timestamp;

use super::{consulted, run_is_pi_native};
use crate::Ctx;

/// One open pi-native container and whether its host binding is healthy.
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

/// Every pi-native worker run and review child of the task, open or closed.
fn containers(task: &Task) -> Vec<(ContainerRef, &Run)> {
    let mut out = Vec::new();
    for run in &task.runs {
        if run_is_pi_native(run) {
            let worker = ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            };
            out.push((worker, run));
        }
        for child in run.review_runs.iter().filter(|c| run_is_pi_native(c)) {
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
                || state
                    .receipts
                    .iter()
                    .any(|r| matches!(r.state, ReceiptState::Reserved | ReceiptState::Uncertain))
        })
}

/// Pure and cheap: whether sync calls `task` for this task at all. A settled closed container
/// never makes it true again.
pub fn wants(task: &Task) -> bool {
    containers(task).iter().any(|(container, run)| {
        container_closure(task, container).is_none() || owes(task, container, run)
    })
}

pub fn task(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<TaskSync> {
    let id = task.id.as_str();
    super::super::effects::reconcile_task(ctx, id)?;
    crate::questions::terminalize(ctx, id)?;
    crate::questions::expire_deadlines(ctx, id, now)?;
    crate::questions::unattended(ctx, id, now)?;
    crate::questions::hold_owner_attention(ctx, id)?;
    super::super::turns::classify(ctx, id, now)?;
    super::super::turns::settle_hold(ctx, id)?;
    let current = ctx.store.get(id)?;
    let uncertain = Duration::from_secs(ctx.store.config.agent.gate_start_uncertain_secs);
    let mut due = super::super::effects::due_gate_wakes(&current, now, uncertain);
    due.extend(super::super::wake::due_wait_wakes(&current));
    super::super::wake::post_due(ctx, &due)?;
    super::super::wake::kickoff(ctx, id)?;
    super::cleanup::run_end(ctx, &current)?;
    settle(ctx, &current, now)?;
    let mut synced = TaskSync::default();
    for (container, _) in containers(&current) {
        if container_closure(&current, &container).is_some() {
            continue;
        }
        consulted("sync");
        let state = container_state(&current, &container);
        let healthy = state.is_some_and(|s| s.binding.health == HostHealth::Healthy);
        if synced.attention.is_none() {
            synced.attention = state.and_then(|s| {
                s.questions
                    .iter()
                    .find(|q| q.recorded_pending() && crate::questions::needs_owner(q))
                    .map(|q| format!("Pi-native question {} needs you: {}", q.key, q.text))
            });
        }
        synced.open.push(ContainerSync { container, healthy });
    }
    if synced.attention.is_none() {
        synced.attention = super::super::turns::attention(&current);
    }
    Ok(synced)
}

/// Records each newly closed container as settled once terminalization cancelled its
/// undelivered Pi messages, run-end cleanup removed its runtime above, and owner delivery has
/// nothing left to retire or finish for it (otherwise the next pass settles it).
fn settle(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<()> {
    let mut closed = Vec::new();
    for (container, run) in containers(task) {
        if run.inbox.settled.is_some() || container_closure(task, &container).is_none() {
            continue;
        }
        let Some(recipient) = crate::questions::recipient(&container) else {
            continue;
        };
        // Late evidence for an orphaned attempt may never come once the host is gone.
        let work = crate::delivery::owner::work(ctx, &recipient)?;
        if !work.unsettled && !work.effects {
            closed.push(recipient);
        }
    }
    if closed.is_empty() {
        return Ok(());
    }
    ctx.store.try_update(&task.id, |current| {
        let mut changed = false;
        for recipient in &closed {
            let state = recipient.state_mut(current)?;
            if state.settled.is_none() {
                state.settled = Some(now);
                changed = true;
            }
        }
        Ok(changed)
    })?;
    Ok(())
}

/// Once per sync pass: the PM wake outbox, only while a Pi-native PM record exists.
pub fn pass(ctx: &Ctx) -> Result<()> {
    if !paths::pm_state_path(&ctx.store).exists() {
        return Ok(());
    }
    consulted("sync");
    super::super::pm_wake::repost(ctx)?;
    Ok(())
}
