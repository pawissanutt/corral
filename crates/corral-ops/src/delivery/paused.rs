//! Pause-aware admission for owner replies and prompts addressed to the shared PM.
use anyhow::Result;
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use corral_core::{Run, Status, Task, store::StoreLock};

pub(crate) use super::pm_reviews::PmReviewTarget;
use crate::Ctx;

/// Attribution, not the command name, distinguishes an owner reply from an automated relay.
pub(crate) fn lift_owner(ctx: &Ctx, recipient: &Recipient) -> Result<()> {
    if ctx.store.event_attribution()?.0 == Some(corral_core::Actor::Owner) {
        crate::session_control::pause::lift_recipient_owner(ctx, recipient)?;
    }
    Ok(())
}

pub(crate) fn lock_pm(ctx: &Ctx) -> Result<corral_core::store::StoreLock> {
    Inbox::new(&ctx.store, Recipient::Pm)?.delivery_lock()
}

/// Keep the input claim alive until sync commits its worker restoration.
pub(crate) struct RestoreAdmission {
    _input: Option<StoreLock>,
}

pub(crate) fn restore_worker(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    to: Status,
) -> Result<Option<RestoreAdmission>> {
    if to != Status::Running || task.status != Status::NeedsMe {
        return Ok(Some(RestoreAdmission { _input: None }));
    }
    let recipient = Recipient::Run {
        task: task.id.clone(),
        run: run.id.clone(),
    };
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let Some(claim) = ctx.store.try_lock(&format!("deliver-{}", inbox.key()))? else {
        return Ok(None);
    };
    if !crate::session_control::automation_may_start(ctx, &recipient)? {
        return Ok(None);
    }
    Ok(Some(RestoreAdmission {
        _input: Some(claim),
    }))
}

/// Serialize only the final prompt with Pause, never PM startup or an outer submission.
/// Submission callers already hold dispatch, so contention must not wait for a delivery
/// that may need dispatch itself. `None` must be queued after that submission returns.
pub(crate) fn prompt_pm(
    ctx: &Ctx,
    pane: &str,
    text: &str,
) -> Result<Option<corral_herdr::Delivery>> {
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let delivery = ctx.store.try_lock(&format!("deliver-{}", inbox.key()))?;
    if delivery.is_none() || crate::session_control::automated_input_must_wait(ctx, &Recipient::Pm)?
    {
        return Ok(None);
    }
    ctx.herdr.agent_prompt(pane, text).map(Some)
}

/// Queue after releasing dispatch/task locks: PM readiness holds the posting lock while
/// validating its binding under dispatch. Posting inside the submission would invert them.
pub(crate) fn queue_review(ctx: &Ctx, text: &str, target: PmReviewTarget) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    inbox.post(Post {
        kind: "review_request",
        urgency: Urgency::Wake,
        author: "corral",
        body: text,
        binding: MessageBinding::default(),
        subject: Some(serde_json::json!({"task": target.task(), "pm_review": target})),
        max_payload: crate::agent::delivery::INPUT_MAX_BYTES,
    })?;
    Ok(())
}
