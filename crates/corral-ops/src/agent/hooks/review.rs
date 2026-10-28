//! Reviewer seam for pi-native review children (T-252 reviewer-role): the launch that binds the
//! parent run, the review child and its callback token, the gate section of the review prompt,
//! and turn classification with OD11. A reviewer turn that ends without a verdict is
//! `no_verdict`; the generation's one `turn_nudge` goes to the child, and the next silent turn
//! escalates the review to Needs me at once, so a healthy host never waits on the idle
//! watchdog's `review_idle_after_secs`.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, AgentRole, AgentRunState, AgentTurn, ClosureReason, ContainerRef, HostHealth,
    ReceiptState, TurnClass, TurnOriginKind, WakeState,
};
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use corral_core::{AutoStage, AutopilotState, ResolvedProfile, Run, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use super::consulted;
use super::surfaces::{GateSection, prompt_file};
use crate::Ctx;
use crate::agent::BindTarget;
use crate::agent::launch::{HostSpec, prepare};
use crate::agent::turns::NUDGE_KIND;

/// The Needs me excerpt prefix of a silent reviewer turn after the generation's nudge. It differs
/// from the worker's, whose hold returns the task to Running once another turn starts.
pub const REVIEW_HOLD_PREFIX: &str = "reviewer turn_ended_without_report: ";

/// The Needs me excerpt prefix of an open review child whose host is lost while its pane stays
/// (recovery); resume returns the task to Review.
pub const LOST_PREFIX: &str = "Pi-native reviewer host lost: ";

/// The heading of the gate section a tiered review prompt carries.
pub const REVIEW_GATE_HEADING: &str = "## Gate contract (review, authoritative)";

const NUDGE: &str = "Your last turn ended without a verdict. If the review is finished, call \
     corral_verdict; if you are waiting on an answer, end your turn after corral_ask; otherwise \
     continue the review. This is the only automatic reminder for this host: the next turn that \
     ends without a verdict goes to the owner.";

/// review-run's preflight, before the review child, prompt or pane exists.
pub fn preflight(ctx: &Ctx, resolved: &ResolvedProfile) -> Result<()> {
    super::preflight::check(ctx, AgentRole::Reviewer, resolved)
}

/// The pane command of the review child review-run just recorded under `parent`, launched in
/// `pane`; `None` for a reviewer of any other harness, which renders its own template.
pub fn command(
    ctx: &Ctx,
    task: &Task,
    parent: &Run,
    resolved: &ResolvedProfile,
    pane: &str,
) -> Option<Result<String>> {
    super::is_pi_native(&resolved.harness).then(|| pi_command(ctx, task, parent, pane))
}

/// The launcher binds the parent run, the review child and the callback token from the saved
/// record, never from the model.
fn pi_command(ctx: &Ctx, task: &Task, parent: &Run, pane: &str) -> Result<String> {
    consulted("review");
    let saved = ctx.store.get(&task.id)?;
    let run = saved
        .runs
        .iter()
        .find(|r| r.id == parent.id)
        .context("the review's parent run disappeared before launch")?;
    let child = run
        .review_runs
        .last()
        .context("review child disappeared before launch")?;
    let callback = run
        .review
        .as_ref()
        .and_then(crate::reviewer::review_callback_token)
        .context("the review has no callback token")?;
    let spec = HostSpec {
        cwd: child.worktree.clone(),
        model: child
            .model
            .clone()
            .context("a pi-native reviewer needs a model")?,
        provider: child
            .model
            .as_deref()
            .and_then(|model| crate::agent::launch::host_provider(child.vendor.as_deref(), model)),
        effort: child.effort.clone(),
        prompt_file: Some(review_prompt_file(ctx, &task.id, &run.id)),
        adopt: false,
    };
    let target = BindTarget::Reviewer {
        task: task.id.clone(),
        run: run.id.clone(),
        review: child.id.clone(),
        callback,
    };
    prepare(ctx, &target, pane, &spec)
}

/// The review prompt review-run wrote for the run's current review.
pub fn review_prompt_file(ctx: &Ctx, task: &str, run: &str) -> std::path::PathBuf {
    prompt_file(ctx, task, &format!("{run}-review"))
}

/// The gate section rendered into the run's review prompt from its GateContract, and the
/// SHA-256 a verdict records as `contract_sha256`; `None` for an untiered review.
pub fn gate_section(ctx: &Ctx, task: &str, run: &str) -> Result<Option<GateSection>> {
    let path = review_prompt_file(ctx, task, run);
    let prompt = match std::fs::read_to_string(&path) {
        Ok(prompt) => prompt,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let Some(start) = prompt
        .match_indices(REVIEW_GATE_HEADING)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || prompt[..*at].ends_with('\n'))
    else {
        return Ok(None);
    };
    let body = &prompt[start + REVIEW_GATE_HEADING.len()..];
    let end = body.find("\n## ").map_or(body.len(), |at| at + 1);
    let text = format!("{REVIEW_GATE_HEADING}{}\n", body[..end].trim_end());
    Ok(Some(GateSection {
        contract_sha256: agent::paths::sha256_hex(text.as_bytes()),
        text,
    }))
}

/// A review child with a healthy binding reports its turns causally, and one whose host is lost
/// waits on `corral agent resume` through `hold_lost`, so the watchdog never waits on its pane's
/// idle status. A gone pane still closes it through the watchdog's process_lost path.
pub fn idle_exempt(child: &Run) -> bool {
    consulted("review");
    child
        .agent_host
        .as_ref()
        .is_some_and(|state| matches!(state.binding.health, HostHealth::Healthy | HostHealth::Lost))
}

/// Whether the open pi-native reviewer is between turns after one that ended without a verdict:
/// superseding its review then interrupts no active turn, and it precedes the turn's nudge.
pub fn between_silent_turns(task: &Task) -> bool {
    consulted("review");
    live_reviewer(task).is_some_and(|container| {
        agent::container_state(task, &container)
            .and_then(latest_silent)
            .is_some()
    })
}

/// Review -> Needs me while the open review child's host is lost and its pane stays. The review
/// stays pending and the child open, so `corral agent resume` can start its next generation, and
/// `release_lost` returns the task to Review once it has. A gone pane is left to the watchdog.
pub fn hold_lost(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let task = ctx.store.get(task_id)?;
    let Some(container) = live_reviewer(&task) else {
        return Ok(false);
    };
    let lost = |task: &Task| {
        live_reviewer(task).as_ref() == Some(&container)
            && agent::container_state(task, &container)
                .is_some_and(|s| s.binding.health == HostHealth::Lost)
    };
    let (Some(state), true) = (agent::container_state(&task, &container), lost(&task)) else {
        return Ok(false);
    };
    let (Some(generation), ContainerRef::Reviewer { review, .. }) =
        (state.current_generation(), &container)
    else {
        return Ok(false);
    };
    if ctx.herdr.pane_get(&generation.pane)?.is_none() {
        return Ok(false);
    }
    consulted("review");
    let n = generation.n;
    let reason = format!(
        "{LOST_PREFIX}{} generation {n}. `corral agent resume {task_id} --review {review}` starts \
         generation {} in the same pane and returns the task to Review; the review stays pending.",
        container.key(),
        n + 1
    );
    let mut batch = None;
    let saved = ctx.store.try_update(task_id, |current| {
        if !lost(current) {
            return Ok(false);
        }
        let run = current
            .current_run_mut()
            .context("the review's run disappeared")?;
        run.blocked_excerpt = Some(reason.clone());
        current.status = Status::NeedsMe;
        batch = Some(crate::owner_notice::record_entry(
            ctx,
            current,
            "native_review_host_lost",
            Timestamp::now(),
            Vec::new(),
        )?);
        Ok(true)
    })?;
    let (Some(saved), Some(batch)) = (saved, batch) else {
        return Ok(false);
    };
    crate::owner_notice::notify(
        ctx,
        crate::owner_notice::newly_committed(&saved, batch)?,
        &format!("{task_id} needs you"),
        &reason,
        true,
    )?;
    let detail = json!({"from": "review", "agent_state": "pi_native", "note": reason});
    ctx.store.log_event(Some(task_id), "needs_me", detail)?;
    Ok(true)
}

/// Returns a task `hold_lost` held in Needs me to Review once its review child is no longer lost
/// (resume fenced the lost generation and opened the next one in the same pane) or once the
/// existing process_lost path closed it because its pane went away. Back in Review, the review
/// watchdog escalates that closed child exactly as it does any reviewer whose pane is gone.
pub fn release_lost(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let released = ctx.store.try_update(task_id, |task| {
        let Some(run) = task
            .current_run()
            .filter(|_| task.status == Status::NeedsMe)
        else {
            return Ok(false);
        };
        let held = run
            .blocked_excerpt
            .as_deref()
            .is_some_and(|e| e.starts_with(LOST_PREFIX));
        let Some(child) = run.review_runs.last().filter(|_| held) else {
            return Ok(false);
        };
        let container = ContainerRef::Reviewer {
            task: task.id.clone(),
            run: run.id.clone(),
            review: child.id.clone(),
        };
        let lost = child
            .agent_host
            .as_ref()
            .is_none_or(|s| s.binding.health == HostHealth::Lost);
        if lost && agent::container_closure(task, &container).is_none() {
            return Ok(false);
        }
        let run = task
            .current_run_mut()
            .context("the review's run vanished")?;
        run.blocked_excerpt = None;
        task.status = Status::Review;
        Ok(true)
    })?;
    Ok(released.is_some())
}

/// The class of an ended reviewer turn from the child's state when it ended, in the plan's
/// precedence order; a turn that committed no verdict and waits on nothing is `no_verdict`.
pub fn classify(
    state: &AgentRunState,
    closure: Option<ClosureReason>,
    turn: &AgentTurn,
) -> TurnClass {
    let reported = state.receipts.iter().any(|r| {
        r.op == "verdict"
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
    } else if crate::agent::turns::cancelled(turn) {
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
        TurnClass::NoVerdict
    }
}

/// The open pi-native review child of the current run of a Review task.
fn live_reviewer(task: &Task) -> Option<ContainerRef> {
    if task.status != Status::Review {
        return None;
    }
    let run = task.current_run()?;
    let child = run.review_runs.last()?;
    child.agent_host.as_ref()?;
    let container = ContainerRef::Reviewer {
        task: task.id.clone(),
        run: run.id.clone(),
        review: child.id.clone(),
    };
    agent::container_closure(task, &container)
        .is_none()
        .then_some(container)
}

/// The latest turn of the current generation when it ended `no_verdict` and none of its turns is
/// open; a lost generation's turn that never ended does not hold a later generation.
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
        .filter(|t| t.generation == state.binding.generation)
        .max_by_key(|t| t.seq)?;
    (latest.class == Some(TurnClass::NoVerdict)).then_some(latest)
}

enum Due {
    Nudge {
        generation: u32,
        seq: u64,
        ended: Timestamp,
    },
    Escalate(String),
}

/// What OD11 owes the open reviewer now: the generation's nudge for its latest silent turn, or
/// the escalation of a later silent turn after the nudge was spent.
fn due(task: &Task, container: &ContainerRef) -> Option<Due> {
    if live_reviewer(task).as_ref() != Some(container) {
        return None;
    }
    let state = agent::container_state(task, container)?;
    let current = state.current_generation()?;
    let latest = latest_silent(state)?;
    let Some(nudged) = current.nudged_at else {
        return Some(Due::Nudge {
            generation: current.n,
            seq: latest.seq,
            ended: latest.ended?,
        });
    };
    let nudged_seq = state
        .turns
        .iter()
        .find(|t| t.generation == current.n && t.ended == Some(nudged))
        .map(|t| t.seq);
    // A nudged turn evicted from the bounded history is older than every recorded turn.
    let later = match nudged_seq {
        Some(seq) => latest.seq > seq,
        None => latest.ended.is_some_and(|ended| ended > nudged),
    };
    later.then(|| {
        Due::Escalate(format!(
            "{REVIEW_HOLD_PREFIX}turn {}/{} of reviewer {} ended without corral_verdict after this \
             host's one automatic nudge",
            latest.generation,
            latest.seq,
            container.key()
        ))
    })
}

/// OD11 for the open pi-native reviewer of a Review task, from sync: posts the generation's one
/// `turn_nudge` (the message and the spent mark commit together), or escalates the review to
/// Needs me after a later silent turn. True when it changed anything.
pub fn nudge(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let task = ctx.store.get(task_id)?;
    let Some(container) = live_reviewer(&task) else {
        return Ok(false);
    };
    consulted("review");
    match due(&task, &container) {
        None => Ok(false),
        Some(Due::Nudge { .. }) => post_nudge(ctx, task_id, &container),
        Some(Due::Escalate(_)) => escalate(ctx, task_id, &container),
    }
}

fn post_nudge(ctx: &Ctx, task_id: &str, container: &ContainerRef) -> Result<bool> {
    let ContainerRef::Reviewer { run, review, .. } = container else {
        return Ok(false);
    };
    let recipient = Recipient::ReviewRun {
        task: task_id.into(),
        run: run.clone(),
        review: review.clone(),
    };
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let _posting = inbox.lock()?;
    let mut posted = None;
    ctx.store.try_update(task_id, |task| {
        let Some(Due::Nudge {
            generation,
            seq,
            ended,
        }) = due(task, container)
        else {
            return Ok(false);
        };
        let key = format!("nudge:{review}:{generation}");
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
        let state =
            agent::container_state_mut(task, container).context("the agent binding disappeared")?;
        state
            .generation_mut(generation)
            .context("the nudged generation disappeared")?
            .nudged_at = Some(ended);
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

/// The idle watchdog's escalation, reached causally: Review -> Needs me with the hold reason,
/// the review left pending for the owner and the host left running in its pane.
fn escalate(ctx: &Ctx, task_id: &str, container: &ContainerRef) -> Result<bool> {
    let now = Timestamp::now();
    let mut escalated = None;
    let mut batch = None;
    let saved = ctx.store.try_update(task_id, |task| {
        let Some(Due::Escalate(reason)) = due(task, container) else {
            return Ok(false);
        };
        let run = task
            .current_run_mut()
            .context("the review's run disappeared")?;
        let sha = run.review.as_ref().map(|r| r.sha.clone());
        run.blocked_excerpt = Some(reason.clone());
        run.autopilot = Some(AutopilotState {
            stage: AutoStage::Escalated,
            reason: Some(reason.clone()),
            sha,
            at: now,
        });
        escalated = Some((run.id.clone(), reason));
        task.status = Status::NeedsMe;
        batch = Some(crate::owner_notice::record_stage(
            ctx,
            task,
            "native_review_watchdog",
            now,
        )?);
        Ok(true)
    })?;
    let Some((run, reason)) = escalated else {
        return Ok(false);
    };
    if let (Some(saved), Some(batch)) = (saved, batch) {
        crate::owner_notice::notify(
            ctx,
            crate::owner_notice::newly_committed(&saved, batch)?,
            &format!("{task_id} needs you"),
            &reason,
            true,
        )?;
    }
    ctx.store.log_event(
        Some(task_id),
        "review_escalated",
        json!({"run": run, "reason": reason, "container": container.key()}),
    )?;
    Ok(true)
}

#[cfg(all(test, unix))]
#[path = "review_role_tests.rs"]
mod tests;
