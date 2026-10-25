//! Provider-stall seam. A pi-native host reports provider errors as `agent_provider_error`
//! events that feed provider_stall directly, so no screen or session-log scan runs for it: the
//! stall is the latest turn of the current generation when that turn ended `provider_failed`
//! with a transient class. provider_stall's own backoff state machine and state file drive it,
//! and each due retry is a bounded `turn_nudge` in the container's inbox. An `auth` failure is
//! never retried; the sync hook sends it to the owner. Owner: T-252 child recovery.
use anyhow::{Context, Result};
use corral_core::agent::{self, AgentRunState, ContainerRef, TurnClass};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{AvailabilityClass, Run, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use super::consulted;
use crate::provider_stall::{BACKOFF_SECS, Drive, Stall, State, Step, advance_state, agent_key};
use crate::{Change, Ctx};

/// Provider error classes the host reports that a later attempt may get past.
const TRANSIENT: [&str; 3] = ["overload", "rate_limit", "provider"];

/// The stall of a container: its current generation's latest turn, ended `provider_failed` with
/// a transient class, and no turn after it.
fn stall(state: &AgentRunState) -> Option<Stall> {
    let n = state.binding.generation;
    let latest = state
        .turns
        .iter()
        .filter(|t| t.generation == n)
        .max_by_key(|t| t.seq)?;
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
fn auth_failed(state: &AgentRunState, container: &ContainerRef) -> Option<String> {
    let n = state.binding.generation;
    let latest = state
        .turns
        .iter()
        .filter(|t| t.generation == n)
        .max_by_key(|t| t.seq)?;
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

fn state_path(ctx: &Ctx, key: &str) -> std::path::PathBuf {
    ctx.store
        .state_dir()
        .join("provider-stall")
        .join(format!("{key}.json"))
}

fn load(ctx: &Ctx, key: &str) -> Option<State> {
    serde_json::from_slice(&std::fs::read(state_path(ctx, key)).ok()?).ok()
}

/// Persists the state where provider_stall's `retrying` and `escalated` read it.
fn store(
    ctx: &Ctx,
    task: &str,
    key: &str,
    before: Option<&State>,
    after: Option<&State>,
) -> Result<()> {
    let path = state_path(ctx, key);
    match (before, after) {
        (Some(before), None) => {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("removing {}", path.display()));
                }
            }
            let since = before.since.to_string();
            ctx.store.log_event(
                Some(task),
                "provider_recovered",
                json!({"key": key, "since": since}),
            )?;
        }
        (_, Some(after)) if before != Some(after) => {
            let dir = path.parent().context("state path has no parent")?;
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            let staged = path.with_extension("tmp");
            std::fs::write(&staged, serde_json::to_vec(after)?)
                .with_context(|| format!("writing {}", staged.display()))?;
            std::fs::rename(&staged, &path)
                .with_context(|| format!("replacing {}", path.display()))?;
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
fn tick(
    ctx: &Ctx,
    task: &Task,
    container: &ContainerRef,
    run: &Run,
    now: Timestamp,
) -> Result<(Step, Option<State>)> {
    consulted("stall");
    let Some(state) = run.agent_host.as_deref() else {
        return Ok((Step::Idle, None));
    };
    let key = agent_key(&task.id, run);
    let before = load(ctx, &key);
    let current = stall(state);
    if before.is_none() && current.is_none() {
        return Ok((Step::Idle, None));
    }
    // Only the current generation's turns count: a lost generation's open turn never ends.
    let n = state.binding.generation;
    let open = state
        .turns
        .iter()
        .any(|t| t.generation == n && t.ended.is_none());
    // A turn in flight (an automatic retry among them) keeps the episode and its spent budget:
    // it is neither a recovery nor a new failure until it ends.
    if open && current.is_none() {
        return Ok((Step::Wait, before));
    }
    let mut after = before.clone();
    let step = advance_state(&mut after, current.as_ref(), !open, now);
    store(ctx, &task.id, &key, before.as_ref(), after.as_ref())?;
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

fn retry(
    ctx: &Ctx,
    task: &Task,
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
        ContainerRef::Worker { run, .. } => Some(run.clone()),
        ContainerRef::Reviewer { review, .. } => Some(review.clone()),
        ContainerRef::Pm => None,
    };
    let key = format!("provider-retry:{}:{generation}:{attempt}", container.key());
    let mut posted = None;
    ctx.store.try_update(&task.id, |current| {
        if agent::container_closure(current, container).is_some() {
            return Ok(false);
        }
        posted = Some(inbox.reserve_in(
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
            .log_event(Some(&task.id), "provider_retry", detail)?;
    }
    Ok(())
}

pub(crate) fn advance_agent(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    now: Timestamp,
) -> Result<Option<Change>> {
    let container = ContainerRef::Worker {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    let (step, state) = tick(ctx, task, &container, run, now)?;
    let (Step::Escalate, Some(state)) = (step, state) else {
        return Ok(None);
    };
    let reason = state.reason();
    let from = task.status;
    let saved = ctx.store.try_update(&task.id, |t| {
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
        Some(&task.id),
        "provider_escalated",
        json!({"key": agent_key(&task.id, run), "reason": reason, "attempts": state.attempts}),
    )?;
    if saved.is_none() || from == Status::NeedsMe {
        return Ok(None);
    }
    ctx.store.log_event(
        Some(&task.id),
        "needs_me",
        json!({"from": from.as_str(), "agent_state": "pi_native", "note": reason}),
    )?;
    Ok(Some(Change {
        task: task.id.clone(),
        from,
        to: Status::NeedsMe,
        note: reason,
        stage: None,
    }))
}

pub(crate) fn advance_reviewer(ctx: &Ctx, task: &str, child: &Run) -> Result<Drive> {
    let record = ctx.store.get(task)?;
    let Some(parent) = record
        .runs
        .iter()
        .find(|r| r.review_runs.iter().any(|c| c.id == child.id))
    else {
        return Ok(Drive::Quiet);
    };
    let container = ContainerRef::Reviewer {
        task: task.into(),
        run: parent.id.clone(),
        review: child.id.clone(),
    };
    if let Some(reason) = child
        .agent_host
        .as_deref()
        .and_then(|s| auth_failed(s, &container))
    {
        let detail = json!({"key": agent_key(task, child), "reason": reason, "class": "auth"});
        ctx.store
            .log_event(Some(task), "provider_escalated", detail)?;
        return Ok(Drive::Escalated(reason));
    }
    Ok(
        match tick(ctx, &record, &container, child, Timestamp::now())? {
            (Step::Escalate, Some(state)) => {
                let detail = json!({"key": agent_key(task, child), "reason": state.reason(),
                "attempts": state.attempts});
                ctx.store
                    .log_event(Some(task), "provider_escalated", detail)?;
                Drive::Escalated(state.reason())
            }
            (Step::Wait | Step::Retry(_), _) => Drive::Holding,
            _ => Drive::Quiet,
        },
    )
}
