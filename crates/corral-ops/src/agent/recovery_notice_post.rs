//! Recovery's one notice to the next generation and the unconfirmed envelope it reports.
use anyhow::{Context, Result};
use corral_core::agent::{self, AgentRunState, ContainerRef};
use corral_core::inbox::{Inbox, Message, Post, Urgency};
use serde_json::{Value, json};

use super::super::bridge::notice::{self, Unconfirmed, binding};
use super::super::launch::{SOCKET_FILE, generation_dir};
use super::{Interrupted, NOTICE_KIND, NOTICE_SOURCE, Relaunch, recipient};
use crate::Ctx;

/// The envelope generation `n` was sent and never confirmed, settled without a resend so the
/// notice is published first.
pub(super) fn settle_unconfirmed(
    ctx: &Ctx,
    container: &ContainerRef,
    n: u32,
    state: &AgentRunState,
) -> Result<Vec<Unconfirmed>> {
    let Some(bound) = state.generation(n).and_then(|g| g.bound.as_ref()) else {
        return Ok(Vec::new());
    };
    let to = recipient(container);
    let endpoint = generation_dir(&ctx.store, &to, n)?.join(SOCKET_FILE);
    notice::settle_unconfirmed(ctx, &to, &endpoint, &bound.session_file)
}

pub(super) fn unconfirmed_json(unconfirmed: &[Unconfirmed]) -> Value {
    let each = |m: &Unconfirmed| json!({"id": m.id, "kind": m.kind, "observed": m.observed});
    Value::Array(unconfirmed.iter().map(each).collect())
}

/// The Pi session ID of the latest generation that completed hello.
pub(super) fn latest_session(state: &AgentRunState) -> Option<String> {
    state
        .generations
        .iter()
        .filter_map(|g| g.bound.as_ref().map(|bound| (g.n, bound)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, bound)| bound.session_id.clone())
}

/// Posts the generation's one notice; `in_progress` receipts are marked in the same commit, so
/// sync later posts exactly one update for each.
pub(super) fn post_notice(
    ctx: &Ctx,
    container: &ContainerRef,
    (lost, next): (u32, u32),
    (interrupted, unconfirmed): (&Interrupted, &[Unconfirmed]),
    rotation: Option<notice::Rotation<'_>>,
    plan: &Relaunch,
) -> Result<Message> {
    let inbox = Inbox::new(&ctx.store, recipient(container))?;
    let run = plan.artifact_run.as_deref();
    let keep = |text: &str| notice::keep_complete(ctx, container, lost, run, text);
    let calls = &interrupted.calls;
    let body = notice::body(
        inbox.key(),
        (lost, next),
        interrupted,
        unconfirmed,
        rotation,
        keep,
    )?;
    let key = format!("recovery:{}/g{lost}", container.key());
    let mut subject = json!({
        "key": key, "lost": lost, "generation": next, "calls": calls,
        "unconfirmed": unconfirmed_json(unconfirmed),
    });
    if !interrupted.scan_limits.is_empty() {
        subject["scan"] = json!({"complete": false, "limits": interrupted.scan_limits});
    }
    if let Some(rotation) = rotation {
        subject["rotation"] = json!({"previous_session": rotation.previous_session});
    }
    let post = || Post {
        kind: NOTICE_KIND,
        urgency: Urgency::Wake,
        author: "corral",
        body: &body,
        binding: binding(container),
        subject: Some(subject.clone()),
        max_payload: corral_core::inbox::MAX_PAYLOAD,
    };
    let Some(task_id) = container.task() else {
        return inbox.post(post());
    };
    let _posting = inbox.lock()?;
    let mut posted = None;
    ctx.store.try_update(task_id, |task| {
        let message = inbox.reserve_in(task, post())?;
        let state =
            agent::container_state_mut(task, container).context("the agent binding disappeared")?;
        for receipt in &mut state.receipts {
            if interrupted.running.contains(&receipt.request_id) {
                let mark = json!({"outcome": "in_progress"});
                crate::review_budget::evidence(
                    receipt,
                    &mark,
                    NOTICE_SOURCE,
                    Some(message.id.clone()),
                );
            }
        }
        posted = Some(message);
        Ok(true)
    })?;
    let message = posted.context("the recovery notice was not reserved")?;
    inbox.announce(&message)?;
    Ok(message)
}
