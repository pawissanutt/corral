//! `corral reply` for runs launched with inbox delivery. An open native question is answered
//! natively and recorded as a question-bound message; everything else is posted and kicked.

use anyhow::{Context, Result, bail, ensure};
use corral_core::Task;
use corral_core::inbox::{
    Inbox, Message, MessageBinding, Receipt, ReceiptLevel, Recipient, TerminalState,
};
use serde_json::json;

use crate::Ctx;
use crate::delivery::owner;

/// The caller holds the task's delivery lock.
pub(super) fn reply(ctx: &Ctx, task: &Task, recipient: &Recipient, text: &str) -> Result<Task> {
    reply_with_policy(ctx, task, recipient, text, None)?;
    ctx.store.get(&task.id)
}

/// Session chat takes the same admission lock and enters the ordinary owner-reply path.
pub(crate) fn session_reply(
    ctx: &Ctx,
    recipient: &Recipient,
    text: &str,
    policy: &str,
) -> Result<(Message, bool)> {
    let id = recipient.task().context("worker reply has no task")?;
    let _delivery = ctx
        .store
        .try_lock(&format!("delivery-{id}"))?
        .context("another reply is being delivered; try again")?;
    let task = ctx.store.get(id).context("reading session reply task")?;
    ensure!(
        owner::live_run(&task).as_ref() == Some(recipient),
        "ended: worker session is no longer the task's live inbox run"
    );
    reply_with_policy(ctx, &task, recipient, text, Some(policy))
}

fn reply_with_policy(
    ctx: &Ctx,
    task: &Task,
    recipient: &Recipient,
    text: &str,
    policy: Option<&str>,
) -> Result<(Message, bool)> {
    let native = match recipient {
        Recipient::Run { .. } => task
            .current_run()
            .is_some_and(crate::agent::hooks::run_is_corral_pi),
        Recipient::PlanRun { .. } => task
            .plan_runs
            .last()
            .is_some_and(|run| run.agent_host.is_some() || corral_core::is_corral_pi(&run.harness)),
        _ => false,
    };
    if native || policy.is_some() {
        crate::session_control::send::validate_text(ctx, recipient, text)?;
    }
    let (run, decision) = match recipient {
        Recipient::Run { run, .. } => (
            run.clone(),
            task.current_run()
                .and_then(|r| r.obligations.pending_decision())
                .map(|d| d.request.id.clone()),
        ),
        Recipient::PlanRun { plan_run, .. } => (plan_run.clone(), None),
        _ => bail!("inbox replies address a coding or planning run"),
    };
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let session_delivery = inbox.delivery_lock().context("locking owner reply input")?;
    let paused = crate::session_control::pause::recipient_paused(ctx, recipient)?;
    if policy.is_some() {
        crate::session_control::pause::lift_recipient_owner(ctx, recipient)?;
    } else {
        crate::delivery::paused::lift_owner(ctx, recipient)?;
    }
    let lifted = paused && !crate::session_control::pause::recipient_paused(ctx, recipient)?;
    let binding = |question_request| MessageBinding {
        run: Some(run.clone()),
        decision: decision.clone(),
        review_round: None,
        question_request,
    };
    // An unsettled native answer keeps the one-reply guard even after its question settled.
    if !native && let Some(pending) = pending_question(ctx, recipient)? {
        ensure!(
            owner::text(&pending) == text,
            "{}: the reply {} to a native question is pending; resend it with `corral reply {} --resend` or settle it with `corral inbox mark` before sending another",
            task.id,
            pending.id,
            task.id
        );
        drop(session_delivery);
        answer(ctx, task, recipient, &pending, true)?;
        return Ok((pending, lifted));
    }
    if !native && let Some(request) = crate::question_reply::route(ctx, task)? {
        ctx.store.log_event(
            Some(&task.id),
            "reply_pending",
            json!({"run": run, "text": text}),
        )?;
        let message = owner::post_with_policy(
            ctx,
            recipient,
            "owner_reply",
            text,
            binding(Some(request)),
            policy,
            |current, _| set_pending(current, recipient, text),
        )?;
        drop(session_delivery);
        answer(ctx, task, recipient, &message, false)?;
        return Ok((message, lifted));
    }
    let message = owner::post_with_policy(
        ctx,
        recipient,
        "owner_reply",
        text,
        binding(None),
        policy,
        |current, _| set_pending(current, recipient, text),
    )?;
    drop(session_delivery);
    let status = owner::kick(ctx, recipient);
    log(ctx, task, recipient, &message, "inbox", status.err(), 0)?;
    Ok((message, lifted))
}

/// `comment --send`: an owner note informs; it is not a reply awaiting an answer.
pub(crate) fn note(ctx: &Ctx, task: &Task, recipient: &Recipient, text: &str) -> Result<Task> {
    crate::delivery::paused::lift_owner(ctx, recipient)?;
    let run = match recipient {
        Recipient::Run { run, .. } => Some(run.clone()),
        _ => None,
    };
    let message = owner::post(
        ctx,
        recipient,
        "owner_note",
        text,
        MessageBinding {
            run,
            ..Default::default()
        },
        |_, _| Ok(()),
    )?;
    let status = owner::kick(ctx, recipient);
    log(ctx, task, recipient, &message, "inbox", status.err(), 0)?;
    ctx.store.get(&task.id)
}

/// `--resend` never re-posts text: it retries the native answer or runs a delivery pass.
pub(super) fn resend(ctx: &Ctx, task: &Task, recipient: &Recipient) -> Result<Task> {
    crate::delivery::paused::lift_owner(ctx, recipient)?;
    if let Some(pending) = pending_question(ctx, recipient)? {
        answer(ctx, task, recipient, &pending, true)?;
        return ctx.store.get(&task.id);
    }
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let mut waiting = false;
    for message in inbox.messages()? {
        waiting |= inbox
            .receipt(message.seq, ReceiptLevel::Observed)?
            .is_none()
            && inbox.terminal(message.seq)?.is_none();
    }
    ensure!(waiting, "{}: no pending inbox message to deliver", task.id);
    if let Err(reason) = owner::kick(ctx, recipient) {
        bail!("{}: delivery pass failed: {reason}", task.id);
    }
    ctx.store.get(&task.id)
}

fn set_pending(task: &mut Task, recipient: &Recipient, text: &str) -> Result<()> {
    let pending = match recipient {
        Recipient::Run { run, .. } => {
            &mut task
                .runs
                .iter_mut()
                .find(|r| &r.id == run && r.outcome.is_none())
                .context("run changed before reply")?
                .pending_reply
        }
        Recipient::PlanRun { plan_run, .. } => {
            &mut task
                .plan_runs
                .iter_mut()
                .find(|r| &r.id == plan_run && r.holds_slot())
                .context("planner changed before reply")?
                .pending_reply
        }
        _ => bail!("inbox replies address a coding or planning run"),
    };
    *pending = Some(text.to_string());
    Ok(())
}

/// The newest unsettled question-bound message: native answers keep the one-reply guard.
fn pending_question(ctx: &Ctx, recipient: &Recipient) -> Result<Option<Message>> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    for message in inbox.messages()?.into_iter().rev() {
        if message.binding.question_request.is_some()
            && inbox
                .receipt(message.seq, ReceiptLevel::Observed)?
                .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            return Ok(Some(message));
        }
    }
    Ok(None)
}

/// A failed native answer stays question-bound: it is never converted into a packed message.
fn answer(
    ctx: &Ctx,
    task: &Task,
    recipient: &Recipient,
    message: &Message,
    resend: bool,
) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _session_delivery = inbox.delivery_lock()?;
    if crate::session_control::automated_input_must_wait(ctx, recipient)? {
        return Ok(());
    }
    let text = owner::text(message);
    let request = message
        .binding
        .question_request
        .as_deref()
        .context("question-bound message has no request")?;
    let delivery = crate::question_reply::answer(
        ctx,
        task,
        text,
        resend,
        &crate::question_reply::Bound {
            request,
            message: &message.id,
        },
    )?;
    drop(_session_delivery);
    if delivery.submitted {
        let inbox = Inbox::new(&ctx.store, recipient.clone())?;
        inbox.record_receipt(
            message.seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: None,
                binding: None,
                observed_at: corral_core::clock::now(),
                via: "omo_question".into(),
                detail: json!({"request": message.binding.question_request}),
            },
        )?;
        let _ = owner::kick(ctx, recipient);
    }
    log(
        ctx,
        task,
        recipient,
        message,
        "omo_question",
        delivery.error.clone(),
        delivery.attempts,
    )?;
    if !delivery.submitted {
        bail!(
            "{}: reply not delivered ({}); pending, retry with `corral reply {} --resend`",
            task.id,
            delivery
                .error
                .as_deref()
                .unwrap_or("native answer was not observed"),
            task.id
        );
    }
    Ok(())
}

fn log(
    ctx: &Ctx,
    task: &Task,
    recipient: &Recipient,
    message: &Message,
    via: &str,
    error: Option<String>,
    attempts: u32,
) -> Result<()> {
    let observed = owner::is_observed(ctx, recipient, message.seq)?;
    let failed = Inbox::new(&ctx.store, recipient.clone())?.terminal(message.seq)?
        == Some(TerminalState::Failed);
    let (kind, target) = match recipient {
        Recipient::PlanRun { .. } => ("plan_replied", Some("plan")),
        _ => ("replied", None),
    };
    ctx.store.log_event(
        Some(&task.id),
        kind,
        json!({
            "target": target, "run": message.binding.run, "text": owner::text(message),
            "by": null, "via": via, "message": message.id, "submitted": observed,
            "failed": failed, "attempts": attempts, "error": error,
        }),
    )
}
