//! Receipt-driven effects: stable receipt journal and task identity fencing.
//! Decision answers commit the journaled receipt (`obligations::answer_reply_with`).

use anyhow::{Context, Result, ensure};
use corral_core::inbox::{EffectStatus, Inbox, Message, Receipt, ReceiptLevel, Recipient};
use corral_core::{ReceiptOutcome, ResumeDelivery, ResumeReceipt, Status, TaskHead, TaskHeadMut};

use crate::Ctx;

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;

/// Locked effects must be replay-safe using the exact receipt supplied here.
pub trait EffectHandler {
    /// Pure preparation. A handler that answers a decision must return its exact receipt here.
    /// The journal freezes it before any effect, including across process interruption.
    fn prepare_decision(
        &self,
        message: &Message,
        receipt: &Receipt,
    ) -> Result<Option<ResumeReceipt>>;
    /// Decision writes must use the supplied journaled receipt, not a newly generated answer.
    fn apply_locked(
        &self,
        ctx: &Ctx,
        message: &Message,
        receipt: &Receipt,
        decision_receipt: Option<&ResumeReceipt>,
    ) -> Result<()>;
    fn apply_task(
        &self,
        task: &mut TaskHeadMut,
        message: &Message,
        receipt: &Receipt,
    ) -> Result<()>;
}

pub fn reconcile(ctx: &Ctx, recipient: &Recipient, handler: &dyn EffectHandler) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _deliver = inbox.delivery_lock()?;
    for message in inbox.unfinished_messages()? {
        let entry = inbox.entry(message.seq)?;
        let Some(observed) = entry.observed else {
            continue;
        };
        let stale = match recipient.task() {
            Some(id) => !current(
                &ctx.store.head(id)?,
                recipient,
                &message,
                entry
                    .effect
                    .as_ref()
                    .and_then(|effect| effect.decision_receipt.as_ref()),
            )?,
            None => false,
        };
        let decision_receipt = match entry.effect {
            Some(effect) => effect.decision_receipt,
            None if stale => None,
            None => {
                let prepared = handler.prepare_decision(&message, &observed)?;
                if let Some(receipt) = &prepared {
                    ensure!(
                        message.binding.decision.as_deref() == Some(receipt.id.as_str())
                            && matches!(recipient, Recipient::Run { run, .. } if *run == receipt.run)
                            && receipt.version == 1
                            && receipt.outcome == ReceiptOutcome::Answered
                            && receipt.delivery == ResumeDelivery::Acknowledged
                            && receipt.observed_at == observed.observed_at
                            && receipt.data.get("inbox_message").and_then(|id| id.as_str())
                                == Some(message.id.as_str())
                            && receipt
                                .answer
                                .as_deref()
                                .is_some_and(|answer| !answer.trim().is_empty()),
                        "prepared decision receipt does not match the observed message binding"
                    );
                }
                prepared
            }
        };
        let effect = inbox.start_effect(message.seq, stale, decision_receipt)?;
        match effect.status {
            EffectStatus::Applied | EffectStatus::SkippedStale => continue,
            EffectStatus::Started => {}
        }
        handler.apply_locked(
            ctx,
            &message,
            &effect.receipt,
            effect.decision_receipt.as_ref(),
        )?;
        let mut became_stale = false;
        if let Some(id) = recipient.task() {
            let _inbox = inbox.lock()?;
            let view = pending_view(&inbox)?;
            ctx.store.update_head(id, |task| {
                let head = task.head();
                if !current(head, recipient, &message, effect.decision_receipt.as_ref())? {
                    became_stale = true;
                    return Ok(());
                }
                if let Some(expected) = &effect.decision_receipt {
                    let run = head
                        .current_run
                        .as_ref()
                        .context("effect run disappeared")?
                        .id
                        .clone();
                    ensure!(
                        head.receipt(&run, &expected.id)?.as_ref() == Some(expected),
                        "locked handler did not commit the prepared decision receipt"
                    );
                }
                if head.effect_applied(recipient, message.seq)? {
                    return Ok(());
                }
                handler.apply_task(task, &message, &effect.receipt)?;
                // A checkpoint request answers nothing, so it leaves blocked state alone.
                let wakes = message.kind != "control_request";
                // current() holds each recipient to its current container.
                let pending = match recipient {
                    Recipient::Run { .. } => {
                        let run = task.current_run_mut().context("effect run disappeared")?;
                        if wakes {
                            run.blocked_excerpt = None;
                            run.seen_working = false;
                            run.idle_since = None;
                            run.idle_no_report = false;
                        }
                        &mut run.pending_reply
                    }
                    Recipient::PlanRun { .. } => {
                        &mut task
                            .last_plan_run_mut()
                            .context("effect plan run disappeared")?
                            .pending_reply
                    }
                    Recipient::PlanReview { .. } => {
                        &mut task
                            .last_plan_mut()
                            .and_then(|plan| plan.current_review.as_mut())
                            .context("effect review disappeared")?
                            .pending_reply
                    }
                    Recipient::ReviewRun { .. } => {
                        &mut task
                            .current_run_mut()
                            .and_then(|run| run.current_child.as_deref_mut())
                            .context("effect review child disappeared")?
                            .pending_reply
                    }
                    Recipient::Pm => anyhow::bail!("PM has no task-local effects"),
                };
                pending.clone_from(&view);
                if wakes
                    && matches!(recipient, Recipient::Run { .. } | Recipient::PlanRun { .. })
                    && task.status == Status::NeedsMe
                {
                    task.status = Status::Running;
                }
                task.mark_effect_applied(recipient, message.seq)
            })?;
        }
        if became_stale {
            inbox.start_effect(message.seq, true, None)?;
            continue;
        }
        inbox.finish_effect(message.seq)?;
    }
    Ok(())
}

/// The newest owner reply or change request that is neither observed nor terminal.
/// Derived from message identities, never from text or from whether an effect applied.
pub(crate) fn pending_view(inbox: &Inbox<'_>) -> Result<Option<String>> {
    for message in inbox.pending_messages()?.into_iter().rev() {
        if matches!(message.kind.as_str(), "owner_reply" | "changes_requested")
            && inbox
                .receipt(message.seq, ReceiptLevel::Observed)?
                .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            return Ok(message
                .body
                .split_once('\n')
                .map(|(_, text)| text.to_string()));
        }
    }
    Ok(None)
}

/// Whether `message` still addresses its recipient's current container, read from the head:
/// projections, the decision by key, and the current review child.
fn current(
    head: &TaskHead,
    recipient: &Recipient,
    message: &Message,
    decision_receipt: Option<&ResumeReceipt>,
) -> Result<bool> {
    if message
        .binding
        .review_round
        .is_some_and(|round| round != head.rounds)
    {
        return Ok(false);
    }
    Ok(match recipient {
        Recipient::Run { run, .. } => {
            let Some(r) = head.current_run.as_ref() else {
                return Ok(false);
            };
            if &r.id != run
                || r.outcome.is_some()
                || r.ended.is_some()
                || message.binding.run.as_ref().is_some_and(|id| id != run)
            {
                return Ok(false);
            }
            match &message.binding.decision {
                None => true,
                Some(id) => head.decision(run, id)?.is_some_and(|d| {
                    d.pending()
                        || decision_receipt.is_some_and(|receipt| {
                            d.receipt.as_ref() == Some(receipt)
                                && receipt.data.get("inbox_message").and_then(|id| id.as_str())
                                    == Some(message.id.as_str())
                        })
                }),
            }
        }
        Recipient::PlanRun { plan_run, .. } => head.last_plan_run.as_ref().is_some_and(|r| {
            &r.id == plan_run
                && r.ended.is_none()
                && matches!(
                    r.state,
                    corral_core::PlanRunState::Planning | corral_core::PlanRunState::Changes
                )
                && message.binding.run.as_ref().is_none_or(|id| id == plan_run)
        }),
        Recipient::PlanReview { plan, review, .. } => head.last_plan.as_ref().is_some_and(|p| {
            &p.id == plan
                && p.current_review.as_ref().is_some_and(|r| {
                    &r.id == review
                        && r.verdict.is_none()
                        && message.binding.run.as_ref().is_none_or(|id| id == review)
                })
        }),
        Recipient::ReviewRun {
            task: id,
            run,
            review,
        } => {
            // Only the current child can be open: the shell holds the run and that child.
            let is_current = head.current_run.as_ref().is_some_and(|r| {
                &r.id == run && r.current_child.as_ref().is_some_and(|c| &c.id == review)
            });
            let child = corral_core::agent::ContainerRef::Reviewer {
                task: id.clone(),
                run: run.clone(),
                review: review.clone(),
            };
            is_current
                && corral_core::agent::container_closure(&head.to_shell_task()?, &child).is_none()
        }
        Recipient::Pm => true,
    })
}
