//! Provider-stall seam. A pi-native host reports provider errors as `agent_provider_error`
//! events that feed provider_stall directly, so no screen or session-log scan runs for it: the
//! stall is the latest turn of the current generation when that turn ended `provider_failed`
//! with a transient class. provider_stall's own backoff state machine and state file drive it,
//! and each due retry is a bounded `turn_nudge` in the container's inbox. An `auth` failure is
//! never retried; the sync hook sends it to the owner. Owner: T-252 child recovery.
use anyhow::{Context, Result};
use corral_core::agent::{self, AgentTurn, ContainerRef, TurnClass};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{AvailabilityClass, RunHead, Status, TaskHead, TaskRef};
use jiff::Timestamp;
use serde_json::json;

use super::consulted;
use crate::provider_stall::{BACKOFF_SECS, Drive, Stall, State, Step, advance_state};
use crate::{Change, Ctx};

/// Provider error classes the host reports that a later attempt may get past.
const TRANSIENT: [&str; 3] = ["overload", "rate_limit", "provider"];

#[cfg(test)]
#[path = "stall_tests.rs"]
mod tests;

fn run_key(task: &str, container: &ContainerRef) -> Result<String> {
    let run = match container {
        ContainerRef::Worker { run, .. } | ContainerRef::Planner { run, .. } => run,
        ContainerRef::Reviewer { review, .. } | ContainerRef::PlanReviewer { review, .. } => review,
        ContainerRef::Pm => anyhow::bail!("provider stalls require a task container"),
    };
    Ok(format!("{task}-{run}"))
}

/// The stall of a container: its current generation's latest turn, ended `provider_failed` with
/// a transient class, and no turn after it.
fn stall(n: u32, latest: Option<&AgentTurn>) -> Option<Stall> {
    let latest = latest?;
    let class = latest.provider_error.as_deref()?;
    let failed = latest.class == Some(TurnClass::ProviderFailed);
    let at = latest
        .ended
        .filter(|_| failed && TRANSIENT.contains(&class))?;
    Some(Stall {
        at,
        message: format!("provider {class} in turn {n}/{}", latest.seq),
        class: AvailabilityClass::Transient {
            retry_after_secs: None,
        },
    })
}

/// A reviewer whose current generation's latest turn failed provider authentication goes to the
/// owner at once, as the worker does through the sync hook: never retried, never a paid fallback.
fn auth_failed(n: u32, latest: Option<&AgentTurn>, container: &ContainerRef) -> Option<String> {
    let latest = latest?;
    let failed = latest.class == Some(TurnClass::ProviderFailed) && latest.ended.is_some();
    (failed && latest.provider_error.as_deref() == Some("auth")).then(|| {
        format!(
            "{}turn {n}/{} of reviewer {}. Run `corral agent login`, then restart the review; no \
             paid fallback is used.",
            super::sync::AUTH_PREFIX,
            latest.seq,
            container.key()
        )
    })
}

const STALLS: corral_core::records::RecordArea<State> =
    corral_core::records::RecordArea::new("provider-stall");

fn load(ctx: &Ctx, key: &str) -> Option<State> {
    STALLS.load(&ctx.store, key).ok().flatten()
}

/// Persists the state where provider_stall's `retrying` and `escalated` read it.
fn store(
    ctx: &Ctx,
    task: &str,
    key: &str,
    before: Option<&State>,
    after: Option<&State>,
) -> Result<()> {
    match (before, after) {
        (Some(before), None) => {
            STALLS.delete(&ctx.store, key)?;
            let since = before.since.to_string();
            ctx.store.log_event(
                Some(task),
                "provider_recovered",
                json!({"key": key, "since": since}),
            )?;
        }
        (_, Some(after)) if before != Some(after) => {
            STALLS.save(&ctx.store, key, after)?;
            if before.map(|s| s.since) != Some(after.since) {
                let detail =
                    json!({"key": key, "since": after.since.to_string(), "message": after.message});
                ctx.store.log_event(Some(task), "provider_stall", detail)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// One sync tick for one container: advances provider_stall's backoff from the Pi turn record,
/// posts a due retry as a `turn_nudge` while no turn is open, and returns the step taken.
fn tick<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    container: &ContainerRef,
    now: Timestamp,
) -> Result<(Step, Option<State>)> {
    consulted("stall");
    let Some(state) = task.agent_head(container)? else {
        return Ok((Step::Idle, None));
    };
    let key = run_key(task.id(), container)?;
    let before = load(ctx, &key);
    let n = state.binding.generation;
    let latest = task.latest_agent_turn(container, n)?;
    let current = stall(n, latest.as_ref());
    if before.is_none() && current.is_none() {
        return Ok((Step::Idle, None));
    }
    // Only the current generation's turns count: a lost generation's open turn never ends.
    let open = task.has_open_agent_turn(container, n)?;
    // A turn in flight (an automatic retry among them) keeps the episode and its spent budget:
    // it is neither a recovery nor a new failure until it ends.
    if open && current.is_none() {
        return Ok((Step::Wait, before));
    }
    let mut after = before.clone();
    let step = advance_state(&mut after, current.as_ref(), !open, now);
    store(ctx, task.id(), &key, before.as_ref(), after.as_ref())?;
    if let (Step::Retry(attempt), Some(stalled)) = (&step, &after) {
        retry(
            ctx,
            task,
            container,
            state.binding.generation,
            *attempt,
            &stalled.message,
        )?;
    }
    Ok((step, after))
}

fn retry<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    container: &ContainerRef,
    generation: u32,
    attempt: u32,
    message: &str,
) -> Result<()> {
    let recipient = crate::questions::recipient(container).context("a Pi-native recipient")?;
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let _posting = inbox.lock()?;
    let body = format!(
        "The {message} and your last turn ended. Continue where you left off. (corral automatic \
         retry {attempt}/{}; when they are used up the task goes to the owner)",
        BACKOFF_SECS.len()
    );
    let run = match container {
        ContainerRef::Worker { run, .. } | ContainerRef::Planner { run, .. } => Some(run.clone()),
        ContainerRef::Reviewer { review, .. } | ContainerRef::PlanReviewer { review, .. } => {
            Some(review.clone())
        }
        ContainerRef::Pm => None,
    };
    let key = format!("provider-retry:{}:{generation}:{attempt}", container.key());
    let mut posted = None;
    ctx.store.try_update_head(task.id(), |current| {
        if agent::container_closure_ref(current, container)?.is_some() {
            return Ok(false);
        }
        posted = Some(inbox.reserve_in_head(
            current,
            Post {
                kind: super::super::turns::NUDGE_KIND,
                urgency: Urgency::Wake,
                author: "corral",
                body: &body,
                binding: MessageBinding {
                    run,
                    ..MessageBinding::default()
                },
                subject: Some(json!({"key": key, "attempt": attempt})),
                max_payload: corral_core::inbox::MAX_PAYLOAD,
            },
        )?);
        Ok(true)
    })?;
    if let Some(message) = posted {
        inbox.announce(&message)?;
        let detail = json!({"key": key, "attempt": attempt, "of": BACKOFF_SECS.len(),
            "message": message.id, "submitted": true});
        ctx.store
            .log_event(Some(task.id()), "provider_retry", detail)?;
    }
    Ok(())
}

pub(crate) fn advance_agent<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    now: Timestamp,
) -> Result<Option<Change>> {
    let container = ContainerRef::Worker {
        task: task.id().clone(),
        run: run.id.clone(),
    };
    let (step, state) = tick(ctx, task, &container, now)?;
    let (Step::Escalate, Some(state)) = (step, state) else {
        return Ok(None);
    };
    let reason = state.reason();
    let from = *task.status();
    let saved = ctx.store.try_update_head(task.id(), |t| {
        if t.status != from {
            return Ok(false);
        }
        let Some(current) = t
            .current_run_mut()
            .filter(|r| r.id == run.id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        current.blocked_excerpt = Some(reason.clone());
        t.status = Status::NeedsMe;
        Ok(true)
    })?;
    ctx.store.log_event(
        Some(task.id()),
        "provider_escalated",
        json!({"key": run_key(task.id(),&container)?, "reason": reason, "attempts": state.attempts}),
    )?;
    if saved.is_none() || from == Status::NeedsMe {
        return Ok(None);
    }
    ctx.store.log_event(
        Some(task.id()),
        "needs_me",
        json!({"from": from.as_str(), "agent_state": "pi_native", "note": reason}),
    )?;
    Ok(Some(Change {
        task: task.id().clone(),
        from,
        to: Status::NeedsMe,
        note: reason,
        stage: None,
    }))
}

pub(crate) fn advance_reviewer(
    ctx: &Ctx,
    task: &TaskHead,
    container: &ContainerRef,
    now: Timestamp,
) -> Result<Drive> {
    let Some(host) = task.agent_head(container)? else {
        return Ok(Drive::Quiet);
    };
    let latest = task.latest_agent_turn(container, host.binding.generation)?;
    let key = run_key(&task.id, container)?;
    if let Some(reason) = auth_failed(host.binding.generation, latest.as_ref(), container) {
        ctx.store.log_event(
            Some(&task.id),
            "provider_escalated",
            json!({"key":key,"reason":reason,"class":"auth"}),
        )?;
        return Ok(Drive::Escalated(reason));
    }
    Ok(match tick(ctx, task, container, now)? {
        (Step::Escalate, Some(state)) => {
            ctx.store.log_event(
                Some(&task.id),
                "provider_escalated",
                json!({"key":key,"reason":state.reason(),"attempts":state.attempts}),
            )?;
            Drive::Escalated(state.reason())
        }
        (Step::Wait | Step::Retry(_), _) => Drive::Holding,
        _ => Drive::Quiet,
    })
}
