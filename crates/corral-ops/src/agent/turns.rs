//! Turn classification on `turn_ended` (T-252 worker-role, OD11). The event op records a host
//! turn's end; the sync hook classifies each ended worker turn once, in one task transaction,
//! from canonical records only (receipts, questions, wakes and the turn's own origin), never
//! from the screen or the transcript. The latest silent turn of a live generation gets the
//! generation's one `turn_nudge`; a later silent turn holds Needs me
//! `turn_ended_without_report` until another turn starts. Turn and run cancellation (row 2 of
//! the plan's table) land with the recovery child, which records corral-issued cancels.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, AgentTurn, ContainerRef, ReceiptState, TurnClass, TurnOriginKind, WakeState,
};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{Status, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// The T-252 child that lands turn classification.
pub const OWNER: &str = "worker-role";
/// The Needs me excerpt prefix of a silent turn after the generation's nudge.
pub const HOLD_PREFIX: &str = "turn_ended_without_report: ";
pub const NUDGE_KIND: &str = "turn_nudge";

const NUDGE: &str = "Your last turn ended without a completion report. If the task is finished, \
     call corral_done; if you are waiting on something, use corral_wait or corral_ask; otherwise \
     continue the task. This is the only automatic reminder for this host: the next turn that \
     ends without a report goes to the owner.";

/// The class of one ended worker turn, in the plan's precedence order.
pub fn classify_turn(task: &Task, container: &ContainerRef, turn: &AgentTurn) -> TurnClass {
    let state = agent::container_state(task, container);
    let reported = state.is_some_and(|s| {
        s.receipts.iter().any(|r| {
            r.op == "done"
                && r.state == ReceiptState::Committed
                && r.identity.generation == turn.generation
                && r.identity.turn == Some(turn.seq)
        })
    });
    if reported {
        TurnClass::Completed
    } else if turn.provider_error.is_some() {
        TurnClass::ProviderFailed
    } else if turn.origin.kind == TurnOriginKind::OwnerTerminal {
        TurnClass::OwnerEngaged
    } else if !super::effects::open_questions(task, container).is_empty() {
        TurnClass::AwaitingAnswer
    } else if state.is_some_and(|s| {
        s.wakes
            .iter()
            .any(|w| matches!(w.state, WakeState::Armed | WakeState::PendingStart))
    }) {
        TurnClass::WaitingExternal
    } else {
        TurnClass::NoReport
    }
}

fn unclassified(turn: &AgentTurn) -> bool {
    turn.ended.is_some() && turn.class.is_none()
}

fn live_worker(task: &Task) -> Option<ContainerRef> {
    let run = task.current_run()?;
    run.agent_host.as_ref()?;
    let container = ContainerRef::Worker {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    agent::container_closure(task, &container)
        .is_none()
        .then_some(container)
}

/// Classifies every ended, unclassified turn of each worker container, posting the OD11 nudge
/// in the same transaction when the latest one is silent. Returns how many turns it classified.
pub fn classify(ctx: &Ctx, task_id: &str, now: Timestamp) -> Result<usize> {
    let task = ctx.store.get(task_id)?;
    let mut classified = 0;
    for container in crate::questions::containers(&task) {
        let ContainerRef::Worker { run, .. } = &container else {
            continue;
        };
        let pending = agent::container_state(&task, &container)
            .is_some_and(|s| s.turns.iter().any(unclassified));
        if !pending {
            continue;
        }
        let recipient = crate::questions::recipient(&container).context("worker recipient")?;
        let inbox = Inbox::new(&ctx.store, recipient)?;
        let _posting = inbox.lock()?;
        let mut posted = None;
        ctx.store.try_update(task_id, |task| {
            let Some(state) = agent::container_state(task, &container) else {
                return Ok(false);
            };
            let classes: Vec<(u32, u64, TurnClass)> = state
                .turns
                .iter()
                .filter(|t| unclassified(t))
                .map(|t| (t.generation, t.seq, classify_turn(task, &container, t)))
                .collect();
            if classes.is_empty() {
                return Ok(false);
            }
            let nudge = nudge_due(task, &container, &classes);
            let state = agent::container_state_mut(task, &container)
                .context("the agent binding disappeared")?;
            for turn in state.turns.iter_mut().filter(|t| unclassified(t)) {
                turn.class = classes
                    .iter()
                    .find(|(g, s, _)| *g == turn.generation && *s == turn.seq)
                    .map(|(_, _, class)| *class);
            }
            classified = classes.len();
            let Some(generation) = nudge else {
                return Ok(true);
            };
            if let Some(entry) = state.generation_mut(generation) {
                entry.nudged_at = Some(now);
            }
            let body = NUDGE.to_owned();
            let key = format!("nudge:{run}:{generation}");
            let message = inbox.reserve_in(
                task,
                Post {
                    kind: NUDGE_KIND,
                    urgency: Urgency::Wake,
                    author: "corral",
                    body: &body,
                    binding: MessageBinding {
                        run: Some(run.clone()),
                        ..MessageBinding::default()
                    },
                    subject: Some(json!({"key": key, "generation": generation})),
                    max_payload: corral_core::inbox::MAX_PAYLOAD,
                },
            )?;
            posted = Some(message);
            Ok(true)
        })?;
        if let Some(message) = posted {
            inbox.announce(&message)?;
        }
    }
    Ok(classified)
}

/// The generation to nudge: the latest newly classified turn is silent, belongs to the live
/// worker's current generation, no later turn is open, the task is Running and the generation
/// has not spent its nudge.
fn nudge_due(
    task: &Task,
    container: &ContainerRef,
    classes: &[(u32, u64, TurnClass)],
) -> Option<u32> {
    let (generation, seq, class) = *classes.iter().max_by_key(|(g, s, _)| (*g, *s))?;
    if class != TurnClass::NoReport || task.status != Status::Running {
        return None;
    }
    if live_worker(task).as_ref() != Some(container) {
        return None;
    }
    let state = agent::container_state(task, container)?;
    let current = state.current_generation()?;
    let later_open = state
        .turns
        .iter()
        .any(|t| t.ended.is_none() && (t.generation, t.seq) > (generation, seq));
    (current.n == generation && current.nudged_at.is_none() && !later_open).then_some(generation)
}

/// Needs me `turn_ended_without_report`: the current worker's latest turn of its current
/// generation ended silent after that generation's nudge was posted. It holds until a new turn
/// starts or another class settles the latest turn.
pub fn attention(task: &Task) -> Option<String> {
    if !matches!(task.status, Status::Running | Status::NeedsMe) {
        return None;
    }
    let container = live_worker(task)?;
    let state = agent::container_state(task, &container)?;
    let current = state.current_generation()?;
    let latest = state
        .turns
        .iter()
        .filter(|t| t.generation == current.n)
        .max_by_key(|t| t.seq)?;
    let ended = latest.ended?;
    let nudged = current.nudged_at?;
    (latest.class == Some(TurnClass::NoReport) && nudged < ended).then(|| {
        format!(
            "{HOLD_PREFIX}turn {}/{} of {} ended without corral_done after this host's one \
             automatic nudge",
            latest.generation,
            latest.seq,
            container.key()
        )
    })
}

/// Returns a task this module held in Needs me to Running once its attention cleared.
pub fn settle_hold(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let settled = ctx.store.try_update(task_id, |task| {
        if task.status != Status::NeedsMe || attention(task).is_some() {
            return Ok(false);
        }
        let Some(run) = task.current_run_mut() else {
            return Ok(false);
        };
        let held = run
            .blocked_excerpt
            .as_deref()
            .is_some_and(|e| e.starts_with(HOLD_PREFIX));
        if !held || run.obligations.pending_decision().is_some() {
            return Ok(false);
        }
        run.blocked_excerpt = None;
        task.status = Status::Running;
        Ok(true)
    })?;
    Ok(settled.is_some())
}

#[cfg(all(test, unix))]
#[path = "worker_role_tests.rs"]
mod tests;
