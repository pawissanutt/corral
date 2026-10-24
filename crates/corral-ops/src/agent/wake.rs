//! Durable worker wakes (T-252 worker-role, F5): one `wait_wake` per wait or gate transition,
//! posted into the run's inbox and marked fired in the same task transaction (`reserve_in` under
//! the inbox lock), keyed `wake:<task>/<run>/<wait-or-attempt>/<state>`. A wake due for a closed
//! container is cancelled instead. Also the run's one `run_prompt` message, which hands a newly
//! bound worker host its prompt file and gate section once.
use anyhow::{Context, Result};
use corral_core::agent::{self, ContainerRef, HostHealth, WakeKind, WakeState};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{Status, Task, WaitState};
use jiff::Timestamp;
use serde_json::json;

use super::effects::DueWake;
use crate::Ctx;

/// The T-252 child that lands wake posting.
pub const OWNER: &str = "worker-role";
pub const WAKE_KIND: &str = "wait_wake";
pub const PROMPT_KIND: &str = "run_prompt";
/// A run prompt message above this size drops the tier's command list.
const PROMPT_COMPACT_BYTES: usize = 6 * 1024;

/// Armed wait subscriptions whose wait left `pending`.
pub fn due_wait_wakes(task: &Task) -> Vec<DueWake> {
    let mut due = Vec::new();
    for run in &task.runs {
        let Some(state) = run.agent_host.as_deref() else {
            continue;
        };
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

fn gate_body(ctx: &Ctx, task: &Task, attempt: &str, reason: &str) -> Result<String> {
    if reason == "gate_start_uncertain" {
        return Ok(format!(
            "corral could not confirm that gate attempt {attempt} started. Read `corral show {} \
             --json`; when no gate is running, call corral_gate again, which adopts the attempt.",
            task.id
        ));
    }
    let readback = crate::gate_tier::gate_readback(ctx, task)?;
    let gate_ref = readback.gate_ref.filter(|r| r.attempt == attempt);
    let section = super::hooks::surfaces::worker_gate_section(ctx, task, false)?;
    let contract = section.ok().map(|s| s.contract_sha256);
    let mut body = format!("Gate attempt {attempt} finished: {reason}.\n");
    if let Some(gate_ref) = &gate_ref {
        body.push_str(&format!("gate_ref: {}\n", serde_json::to_string(gate_ref)?));
    }
    if let Some(contract) = &contract {
        body.push_str(&format!("contract_sha256: {contract}\n"));
    }
    body.push_str(
        "When it passed and the task is complete, call corral_done with this gate_ref and \
         contract_sha256. Otherwise fix the failure inside the module scope, commit, rebase and \
         call corral_gate again.",
    );
    Ok(body)
}

fn wait_body(task: &Task, run: &str, wait: &str, reason: &str) -> String {
    let record = task
        .runs
        .iter()
        .find(|r| r.id == run)
        .and_then(|r| r.obligations.waits.iter().find(|w| w.request.id == wait));
    match record {
        Some(record) => format!(
            "Wait {wait} ({}) reached {reason}: {}",
            record.request.condition, record.observation.detail
        ),
        None => format!("Wait {wait} reached {reason}."),
    }
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
        let snapshot = ctx.store.get(task_id)?;
        let Some(subscription) = agent::container_state(&snapshot, &wake.container)
            .and_then(|s| {
                s.wakes
                    .iter()
                    .find(|w| w.request_id == wake.request_id && w.state == WakeState::Armed)
            })
            .cloned()
        else {
            continue;
        };
        let body = match subscription.kind {
            WakeKind::Gate => gate_body(ctx, &snapshot, &subscription.target, &wake.reason)?,
            WakeKind::Wait => wait_body(&snapshot, run, &subscription.target, &wake.reason),
        };
        let key = format!(
            "wake:{}/{}/{}",
            wake.container.key(),
            subscription.target,
            wake.reason
        );
        let mut message = None;
        ctx.store.try_update(task_id, |task| {
            let closed = agent::container_closure(task, &wake.container).is_some();
            let armed = agent::container_state(task, &wake.container).is_some_and(|s| {
                s.wakes
                    .iter()
                    .any(|w| w.request_id == wake.request_id && w.state == WakeState::Armed)
            });
            if !armed {
                return Ok(false);
            }
            let reserved = if closed {
                None
            } else {
                Some(inbox.reserve_in(
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
            let state = agent::container_state_mut(task, &wake.container)
                .context("the agent binding disappeared")?;
            let stored = state
                .wakes
                .iter_mut()
                .find(|w| w.request_id == wake.request_id && w.state == WakeState::Armed)
                .context("the wake subscription disappeared")?;
            match &reserved {
                Some(m) => {
                    stored.key = key.clone();
                    stored.state = WakeState::Fired;
                    stored.fired_at = Some(Timestamp::now());
                    stored.message_id = Some(m.id.clone());
                }
                None => stored.state = WakeState::Cancelled,
            }
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

/// The prompt file dispatch wrote for the run, the one the host's bootstrap names.
fn prompt_file(ctx: &Ctx, task: &str, run: &str) -> std::path::PathBuf {
    ctx.store.prompts_dir().join(format!("{task}-{run}.md"))
}

fn prompt_body(ctx: &Ctx, task: &Task, run: &str, compact: bool) -> Result<String> {
    let path = prompt_file(ctx, &task.id, run);
    let mut body = format!(
        "Read {} in full and follow it: that file is your complete prompt for {} run {run}. \
         Wherever it names a corral command that changes state (done, gate, wait, run progress, \
         run checkpoint, evidence record, artifact add, a question), call the matching corral_* \
         tool instead.\n\n",
        path.display(),
        task.id
    );
    match super::hooks::surfaces::worker_gate_section(ctx, task, !compact)? {
        Ok(section) => {
            body.push_str(&section.text);
            body.push_str(&format!(
                "\ncontract_sha256: {} (pass it to corral_done)\n",
                section.contract_sha256
            ));
        }
        Err(reason) => body.push_str(&format!(
            "## Gate\n\ncorral cannot resolve this run's gate contract now ({reason}); call \
             corral_gate once after the final rebase and follow the result it returns.\n"
        )),
    }
    Ok(body)
}

/// Posts the run prompt message once per run, as soon as the current worker generation is
/// bound and healthy; the host submits it as the run's first turn. Returns whether it posted.
pub fn kickoff(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let task = ctx.store.get(task_id)?;
    let Some(run) = task.current_run() else {
        return Ok(false);
    };
    let container = ContainerRef::Worker {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    let ready = task.status == Status::Running
        && agent::container_closure(&task, &container).is_none()
        && run.agent_host.as_ref().is_some_and(|s| {
            s.binding.health == HostHealth::Healthy
                && s.current_generation().is_some_and(|g| g.bound.is_some())
        })
        && prompt_file(ctx, &task.id, &run.id).is_file();
    if !ready {
        return Ok(false);
    }
    let run_id = run.id.clone();
    let recipient = crate::questions::recipient(&container).context("worker recipient")?;
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let _posting = inbox.lock()?;
    if inbox.messages()?.iter().any(|m| m.kind == PROMPT_KIND) {
        return Ok(false);
    }
    let mut body = prompt_body(ctx, &task, &run_id, false)?;
    if body.len() > PROMPT_COMPACT_BYTES {
        body = prompt_body(ctx, &task, &run_id, true)?;
    }
    let mut message = None;
    ctx.store.try_update(task_id, |task| {
        if task.current_run().map(|r| r.id.as_str()) != Some(run_id.as_str()) {
            return Ok(false);
        }
        message = Some(inbox.reserve_in(
            task,
            Post {
                kind: PROMPT_KIND,
                urgency: Urgency::Wake,
                author: "corral",
                body: &body,
                binding: MessageBinding {
                    run: Some(run_id.clone()),
                    ..MessageBinding::default()
                },
                subject: Some(json!({"key": format!("prompt:{task_id}/{run_id}")})),
                max_payload: super::delivery::INPUT_MAX_BYTES,
            },
        )?);
        Ok(true)
    })?;
    match message {
        Some(message) => {
            inbox.announce(&message)?;
            Ok(true)
        }
        None => Ok(false),
    }
}
