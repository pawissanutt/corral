//! Durable worker wakes (T-252 worker-role, F5): one `wait_wake` per wait or gate transition,
//! posted into the run's inbox and marked fired in the same task transaction (`reserve_in` under
//! the inbox lock), keyed `wake:<task>/<run>/<wait-or-attempt>/<state>`. A wake due for a closed
//! container is cancelled instead.
use anyhow::{Context, Result};
use corral_core::agent::{self, ContainerRef, WakeKind, WakeState};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{Task, TaskRef, WaitState};
use serde_json::json;

use super::effects::DueWake;
use crate::Ctx;

/// The T-252 child that lands wake posting.
pub const OWNER: &str = "worker-role";
pub const WAKE_KIND: &str = "wait_wake";

/// Armed wait subscriptions whose wait left `pending`. A run that released its coding slot (parked
/// on another task, or woken and queued for a slot) keeps its wakes owed until it holds a slot.
pub fn due_wait_wakes(task: &Task) -> Vec<DueWake> {
    let mut due = Vec::new();
    for run in &task.runs {
        let Some(state) = run.agent_host.as_deref() else {
            continue;
        };
        if run.releases_slot(&task.id) {
            continue;
        }
        let armed = state
            .wakes
            .iter()
            .filter(|w| w.kind == WakeKind::Wait && w.state == WakeState::Armed);
        for wake in armed {
            let wait = run
                .obligations
                .waits
                .iter()
                .find(|w| w.request.id == wake.target);
            if let Some(wait) = wait.filter(|w| w.observation.state != WaitState::Pending) {
                due.push(DueWake {
                    container: ContainerRef::Worker {
                        task: task.id.clone(),
                        run: run.id.clone(),
                    },
                    request_id: wake.request_id.clone(),
                    reason: wait.observation.state.as_str().to_owned(),
                });
            }
        }
    }
    due
}

pub(crate) fn due_wait_wakes_ref<T: TaskRef + ?Sized>(task: &T) -> Result<Vec<DueWake>> {
    let mut due = Vec::new();
    for (container, wake) in task.active_worker_wakes()? {
        if wake.kind != WakeKind::Wait || wake.state != WakeState::Armed {
            continue;
        }
        let ContainerRef::Worker { run, .. } = &container else {
            continue;
        };
        if task
            .run_head(run)?
            .is_some_and(|run| run.releases_slot(task.id()))
        {
            continue;
        }
        if let Some(wait) = task
            .wait(run, &wake.target)?
            .filter(|wait| wait.observation.state != WaitState::Pending)
        {
            due.push(DueWake {
                container,
                request_id: wake.request_id,
                reason: wait.observation.state.as_str().to_owned(),
            });
        }
    }
    Ok(due)
}

fn gate_body<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    attempt: &str,
    reason: &str,
) -> Result<String> {
    if reason == "not_started" {
        let receipt = task.worker_queued_effect_receipt(attempt)?;
        return Ok(format!(
            "Queued gate attempt {attempt} did not start: {}. Resolve the cause and call corral_gate again.",
            receipt
                .as_ref()
                .and_then(|r| r.result["reason"].as_str())
                .unwrap_or("admission refused")
        ));
    }
    if reason == "gate_start_uncertain" {
        return Ok(format!(
            "corral could not confirm that gate attempt {attempt} started. Read `corral show {} \
             --json`; when no gate is running, call corral_gate again, which adopts the attempt.",
            task.id()
        ));
    }
    let readback = crate::gate_tier::gate_readback_ref(ctx, task)?;
    let gate_ref = readback.gate_ref.filter(|r| r.attempt == attempt);
    let section = super::hooks::surfaces::worker_gate_section_ref(ctx, task)?;
    let contract = section.map(|s| s.contract_sha256);
    let mut body = format!("Gate attempt {attempt} finished: {reason}.\n");
    if let Some(gate_ref) = &gate_ref {
        body.push_str(&format!("gate_ref: {}\n", serde_json::to_string(gate_ref)?));
    }
    if let Some(contract) = &contract {
        body.push_str(&format!("contract_sha256: {contract}\n"));
    }
    if reason == "stale" {
        let current = task.current_run_head()?;
        let detail = current
            .as_ref()
            .and_then(|run| run.gate.as_ref())
            .and_then(|gate| gate.failed_cmd.as_deref())
            .unwrap_or("verification inputs changed");
        body.push_str(&format!("Commands passed, but the receipt is stale: {detail}. This is not a command failure and cannot authorize done or approve. Rebase onto current main, then call corral_gate once on the new base."));
        return Ok(body);
    }
    body.push_str(
        "When it passed and the task is complete, call corral_done with this gate_ref and \
         contract_sha256. Otherwise fix the failure inside the module scope, commit, rebase and \
         call corral_gate again.",
    );
    Ok(body)
}

fn wait_body<T: TaskRef + ?Sized>(task: &T, run: &str, wait: &str, reason: &str) -> Result<String> {
    let record = task.wait(run, wait)?;
    Ok(match record {
        Some(record) => format!(
            "Wait {wait} ({}) reached {reason}: {}",
            record.request.condition, record.observation.detail
        ),
        None => format!("Wait {wait} reached {reason}."),
    })
}

/// Posts the wakes sync found due and returns how many were posted. A wake posts at most once:
/// the message and the subscription's `fired` mark commit together.
pub fn post_due(ctx: &Ctx, due: &[DueWake]) -> Result<usize> {
    let mut posted = 0;
    for wake in due {
        let task_id = wake.container.task().context("wakes live on worker runs")?;
        let ContainerRef::Worker { run, .. } = &wake.container else {
            continue;
        };
        let recipient = crate::questions::recipient(&wake.container).context("worker recipient")?;
        let inbox = Inbox::new(&ctx.store, recipient)?;
        let _posting = inbox.lock()?;
        let snapshot = ctx.store.head(task_id)?;
        let Some(subscription) = snapshot
            .agent_wake(&wake.container, &wake.request_id)?
            .filter(|wake| wake.state == WakeState::Armed)
        else {
            continue;
        };
        let body = match subscription.kind {
            WakeKind::Gate => gate_body(ctx, &snapshot, &subscription.target, &wake.reason)?,
            WakeKind::Wait => wait_body(&snapshot, run, &subscription.target, &wake.reason)?,
        };
        let key = format!(
            "wake:{}/{}/{}",
            wake.container.key(),
            subscription.target,
            wake.reason
        );
        let mut message = None;
        ctx.store.try_update_head(task_id, |task| {
            let closed = agent::container_closure_ref(task, &wake.container)?.is_some();
            let Some(mut stored) = task
                .agent_wake(&wake.container, &wake.request_id)?
                .filter(|wake| wake.state == WakeState::Armed)
            else {
                return Ok(false);
            };
            let queued = task
                .run_head(run)?
                .is_some_and(|current| current.releases_slot(&task.id));
            if queued && !closed {
                return Ok(false);
            }
            let reserved = if closed {
                None
            } else {
                Some(inbox.reserve_in_head(
                    task,
                    Post {
                        kind: WAKE_KIND,
                        urgency: Urgency::Wake,
                        author: "corral",
                        body: &body,
                        binding: MessageBinding {
                            run: Some(run.clone()),
                            ..MessageBinding::default()
                        },
                        subject: Some(json!({
                            "key": key, "kind": subscription.kind, "target": subscription.target,
                            "state": wake.reason,
                        })),
                        max_payload: corral_core::inbox::MAX_PAYLOAD,
                    },
                )?)
            };
            match &reserved {
                Some(m) => {
                    stored.key = key.clone();
                    stored.state = WakeState::Fired;
                    stored.fired_at = Some(ctx.store.now());
                    stored.message_id = Some(m.id.clone());
                }
                None => stored.state = WakeState::Cancelled,
            }
            task.put_agent_record(&wake.container, agent::NativeRecord::Wake(stored))?;
            message = reserved;
            Ok(true)
        })?;
        if let Some(message) = message {
            inbox.announce(&message)?;
            ctx.store.log_event(
                Some(task_id),
                "agent_wake_posted",
                json!({"container": wake.container.key(), "key": key, "message": message.id}),
            )?;
            posted += 1;
        }
    }
    Ok(posted)
}

#[cfg(all(test, unix))]
#[path = "wake_head_tests.rs"]
mod head_tests;

#[cfg(all(test, unix))]
#[path = "worker_lane_tests.rs"]
mod tests;
