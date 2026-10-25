//! Turn classification on `turn_ended` (T-252 worker-role, OD11). The event op classifies an
//! ended worker turn inside its own turn_ended transaction, from the container's canonical state
//! at that moment (receipts, questions, wakes and the turn's own origin), never from the screen
//! or the transcript, so a later question or wake cannot reclassify it. The sync hook then gives
//! the latest silent turn of a live generation that generation's one `turn_nudge`, and holds a
//! later silent turn in Needs me `turn_ended_without_report` until another turn starts. A turn
//! `corral agent cancel` marked (recovery) classifies `cancelled`, row 2 of the plan's table.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, AgentRole, AgentRunState, AgentTurn, ClosureReason, ContainerRef, ReceiptState,
    TurnClass, TurnOriginKind, WakeState,
};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{Status, Task};
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

/// Row 2: `corral agent cancel` marked this turn cancelled, before it asked the host or once the
/// host confirmed cancelling it (recovery, `bridge::cancel`).
pub fn cancelled(turn: &AgentTurn) -> bool {
    turn.ended.is_some() && turn.class == Some(TurnClass::Cancelled)
}

/// The class of an ended worker turn from the container's state when it ended, in the plan's
/// precedence order. `closure` is the container's closure read in the same transaction.
pub fn class_at_end(
    state: &AgentRunState,
    closure: Option<ClosureReason>,
    turn: &AgentTurn,
) -> TurnClass {
    let reported = state.receipts.iter().any(|r| {
        r.op == "done"
            && r.state == ReceiptState::Committed
            && r.identity.generation == turn.generation
            && r.identity.turn == Some(turn.seq)
    });
    let asking = closure.is_none() && state.questions.iter().any(|q| q.recorded_pending());
    let waiting = state
        .wakes
        .iter()
        .any(|w| matches!(w.state, WakeState::Armed | WakeState::PendingStart));
    if reported {
        TurnClass::Completed
    } else if cancelled(turn) {
        TurnClass::Cancelled
    } else if turn.provider_error.is_some() {
        TurnClass::ProviderFailed
    } else if turn.origin.kind == TurnOriginKind::OwnerTerminal {
        TurnClass::OwnerEngaged
    } else if asking {
        TurnClass::AwaitingAnswer
    } else if waiting {
        TurnClass::WaitingExternal
    } else {
        TurnClass::NoReport
    }
}

/// Classifies turn `index` of a worker or review child in the transaction that ended it.
pub fn classify_ended(state: &mut AgentRunState, closure: Option<ClosureReason>, index: usize) {
    let class = match state.binding.role {
        AgentRole::Worker => class_at_end(state, closure, &state.turns[index]),
        AgentRole::Reviewer => super::hooks::review::classify(state, closure, &state.turns[index]),
        AgentRole::Pm => return,
    };
    state.turns[index].class = Some(class);
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

/// The latest turn of the current generation when it has ended silent and none of its turns is
/// open. A lost generation's turn that was open when its host died never ends; it stays
/// recorded as it was and does not hold a later generation's decisions.
fn latest_silent(state: &AgentRunState) -> Option<&AgentTurn> {
    let generation = state.binding.generation;
    if state
        .turns
        .iter()
        .any(|t| t.generation == generation && t.ended.is_none())
    {
        return None;
    }
    let latest = state
        .turns
        .iter()
        .filter(|t| t.generation == generation)
        .max_by_key(|t| t.seq)?;
    (latest.class == Some(TurnClass::NoReport)).then_some(latest)
}

/// The turn that spent the generation's nudge: `nudged_at` records that turn's own end, so
/// the identity is causal, not the time some sync pass happened to run.
fn nudged_turn(state: &AgentRunState) -> Option<(Option<u64>, jiff::Timestamp)> {
    let generation = state.current_generation()?;
    let nudged = generation.nudged_at?;
    let turn = state
        .turns
        .iter()
        .find(|t| t.generation == generation.n && t.ended == Some(nudged));
    Some((turn.map(|t| t.seq), nudged))
}

/// Posts the OD11 nudge for the live worker's latest silent turn when the task is Running and
/// its generation has not spent the nudge; the message and the spent mark commit together.
pub fn nudge(ctx: &Ctx, task_id: &str) -> Result<bool> {
    if super::hooks::review::nudge(ctx, task_id)? {
        return Ok(true);
    }
    let task = ctx.store.get(task_id)?;
    let Some(container) = live_worker(&task) else {
        return Ok(false);
    };
    let ContainerRef::Worker { run, .. } = &container else {
        return Ok(false);
    };
    let due = |task: &Task| -> Option<(u32, u64, jiff::Timestamp)> {
        if task.status != Status::Running || live_worker(task).as_ref() != Some(&container) {
            return None;
        }
        let state = agent::container_state(task, &container)?;
        let current = state.current_generation()?;
        let latest = latest_silent(state)?;
        (current.nudged_at.is_none()).then_some((current.n, latest.seq, latest.ended?))
    };
    if due(&task).is_none() {
        return Ok(false);
    }
    let recipient = crate::questions::recipient(&container).context("worker recipient")?;
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let _posting = inbox.lock()?;
    let mut posted = None;
    ctx.store.try_update(task_id, |task| {
        let Some((generation, seq, ended)) = due(task) else {
            return Ok(false);
        };
        let key = format!("nudge:{run}:{generation}");
        let message = inbox.reserve_in(
            task,
            Post {
                kind: NUDGE_KIND,
                urgency: Urgency::Wake,
                author: "corral",
                body: NUDGE,
                binding: MessageBinding {
                    run: Some(run.clone()),
                    ..MessageBinding::default()
                },
                subject: Some(json!({"key": key, "generation": generation, "turn": seq})),
                max_payload: corral_core::inbox::MAX_PAYLOAD,
            },
        )?;
        let state = agent::container_state_mut(task, &container)
            .context("the agent binding disappeared")?;
        let entry = state
            .generation_mut(generation)
            .context("the nudged generation disappeared")?;
        entry.nudged_at = Some(ended);
        posted = Some(message);
        Ok(true)
    })?;
    match posted {
        Some(message) => {
            inbox.announce(&message)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Needs me `turn_ended_without_report`: the current worker's latest turn ended silent, and it
/// is a later turn than the one that spent this generation's nudge. It holds until a new turn
/// starts or a reported turn settles the latest one.
pub fn attention(task: &Task) -> Option<String> {
    if !matches!(task.status, Status::Running | Status::NeedsMe) {
        return None;
    }
    let container = live_worker(task)?;
    let state = agent::container_state(task, &container)?;
    let latest = latest_silent(state)?;
    let (nudged_seq, nudged_end) = nudged_turn(state)?;
    // A nudged turn evicted from the bounded history is older than every recorded turn.
    let later = match nudged_seq {
        Some(seq) => latest.seq > seq,
        None => latest.ended.is_some_and(|ended| ended > nudged_end),
    };
    later.then(|| {
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
