//! `corral reply` for runs launched with inbox delivery. An open native question is answered
//! natively and recorded as a question-bound message; everything else is posted and kicked.

use anyhow::{Context, Result, bail, ensure};
use corral_core::Task;
use corral_core::inbox::{
    Inbox, Message, MessageBinding, Receipt, ReceiptLevel, Recipient, TerminalState,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::delivery::owner;

/// The caller holds the task's delivery lock.
pub(super) fn reply(ctx: &Ctx, task: &Task, recipient: &Recipient, text: &str) -> Result<Task> {
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
    let binding = |question_request| MessageBinding {
        run: Some(run.clone()),
        decision: decision.clone(),
        review_round: None,
        question_request,
    };
    if let Some(request) = crate::question_reply::route(ctx, task)? {
        if let Some(pending) = pending_question(ctx, recipient)? {
            ensure!(
                owner::text(&pending) == text,
                "{}: reply to the open question is pending; resend it with `corral reply {} --resend` before sending another",
                task.id,
                task.id
            );
            return answer(ctx, task, recipient, &pending, true);
        }
        ctx.store.log_event(
            Some(&task.id),
            "reply_pending",
            json!({"run": run, "text": text}),
        )?;
        let message = owner::post(
            ctx,
            recipient,
            "owner_reply",
            text,
            binding(Some(request)),
            |current, _| set_pending(current, recipient, text),
        )?;
        return answer(ctx, task, recipient, &message, false);
    }
    let message = owner::post(
        ctx,
        recipient,
        "owner_reply",
        text,
        binding(None),
        |current, _| set_pending(current, recipient, text),
    )?;
    let status = owner::kick(ctx, recipient);
    log(ctx, task, recipient, &message, "inbox", status.err(), 0)?;
    ctx.store.get(&task.id)
}

/// `comment --send`: an owner note informs; it is not a reply awaiting an answer.
pub(crate) fn note(ctx: &Ctx, task: &Task, recipient: &Recipient, text: &str) -> Result<Task> {
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
    if let Some(pending) = pending_question(ctx, recipient)? {
        return answer(ctx, task, recipient, &pending, true);
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
) -> Result<Task> {
    let text = owner::text(message);
    let delivery = crate::question_reply::answer(ctx, task, text, resend)?;
    if delivery.submitted {
        let inbox = Inbox::new(&ctx.store, recipient.clone())?;
        inbox.record_receipt(
            message.seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: None,
                binding: None,
                observed_at: Timestamp::now(),
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
    ctx.store.get(&task.id)
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
