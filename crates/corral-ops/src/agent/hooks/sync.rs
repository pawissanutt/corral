//! Sync seam: one hook call per task that holds live pi-native work, and one per open
//! container inside it. It persists F4 question terminalization, owner deadlines, the
//! unattended rule, gate reservations and uncertain gate starts, due wakes and run-end
//! cleanup, and reports each open container's binding health so sync leaves
//! `idle_no_report` unset for a healthy host. Owner: T-252 children worker-role (wakes, turns)
//! and recovery (host loss).
use std::time::Duration;

use anyhow::Result;
use corral_core::Task;
use corral_core::agent::{ContainerRef, HostHealth, container_closure, container_state, paths};
use jiff::Timestamp;

use super::{consulted, run_is_pi_native};
use crate::Ctx;

/// One open pi-native container and whether its host binding is healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSync {
    pub container: ContainerRef,
    pub healthy: bool,
}

/// Every pi-native worker run and review child of the task, open or closed.
fn containers(task: &Task) -> Vec<ContainerRef> {
    let mut out = Vec::new();
    for run in &task.runs {
        if run_is_pi_native(run) {
            out.push(ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            });
        }
        for child in run.review_runs.iter().filter(|c| run_is_pi_native(c)) {
            out.push(ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            });
        }
    }
    out
}

/// Whether a closed container still holds something sync must settle.
fn unsettled(task: &Task, container: &ContainerRef) -> bool {
    container_state(task, container).is_some_and(|state| {
        state.questions.iter().any(|q| q.recorded_pending())
            || state.receipts.iter().any(|r| {
                matches!(
                    r.state,
                    corral_core::agent::ReceiptState::Reserved
                        | corral_core::agent::ReceiptState::Uncertain
                )
            })
    })
}

/// Pure and cheap: whether sync calls `task` for this task at all.
pub fn wants(task: &Task) -> bool {
    containers(task)
        .iter()
        .any(|container| container_closure(task, container).is_none() || unsettled(task, container))
}

pub fn task(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<Vec<ContainerSync>> {
    let id = task.id.as_str();
    super::super::effects::reconcile_task(ctx, id)?;
    crate::questions::terminalize(ctx, id)?;
    crate::questions::expire_deadlines(ctx, id, now)?;
    crate::questions::unattended(ctx, id, now)?;
    let current = ctx.store.get(id)?;
    let uncertain = Duration::from_secs(ctx.store.config.agent.gate_start_uncertain_secs);
    let due = super::super::effects::due_gate_wakes(&current, now, uncertain);
    super::super::wake::post_due(ctx, &due)?;
    super::cleanup::run_end(ctx, &current)?;
    let mut open = Vec::new();
    for container in containers(&current) {
        if container_closure(&current, &container).is_some() {
            continue;
        }
        consulted("sync");
        let healthy = container_state(&current, &container)
            .is_some_and(|state| state.binding.health == HostHealth::Healthy);
        open.push(ContainerSync { container, healthy });
    }
    Ok(open)
}

/// Whether the task's current worker run is open with a healthy binding.
pub fn current_worker_healthy(task: &Task, open: &[ContainerSync]) -> bool {
    task.current_run().is_some_and(|run| {
        open.iter().any(|c| {
            c.healthy
                && matches!(&c.container, ContainerRef::Worker { run: id, .. } if *id == run.id)
        })
    })
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
