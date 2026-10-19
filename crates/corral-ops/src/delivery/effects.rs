//! Effect skeleton: stable receipt journal and task identity fencing.
//! The obligations receipt bridge is deliberately left to phase 2.

use anyhow::{Context, Result};
use corral_core::inbox::{EffectStatus, Inbox, Message, Receipt, ReceiptLevel, Recipient};
use corral_core::{Status, Task};

use crate::Ctx;

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;

/// Locked effects must be replay-safe using the exact receipt supplied here.
pub trait EffectHandler {
    fn apply_locked(&self, ctx: &Ctx, message: &Message, receipt: &Receipt) -> Result<()>;
    fn apply_task(&self, task: &mut Task, message: &Message, receipt: &Receipt) -> Result<()>;
}

pub fn reconcile(ctx: &Ctx, recipient: &Recipient, handler: &dyn EffectHandler) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _deliver = inbox.delivery_lock()?;
    for message in inbox.messages()? {
        if inbox
            .receipt(message.seq, ReceiptLevel::Observed)?
            .is_none()
        {
            continue;
        }
        let stale = recipient
            .task()
            .map(|id| ctx.store.get(id).map(|t| !current(&t, recipient, &message)))
            .transpose()?
            .unwrap_or(false);
        let effect = inbox.start_effect(message.seq, stale)?;
        match effect.status {
            EffectStatus::Applied | EffectStatus::SkippedStale => continue,
            EffectStatus::Started => {}
        }
        handler.apply_locked(ctx, &message, &effect.receipt)?;
        let mut became_stale = false;
        if let Some(id) = recipient.task() {
            let _inbox = inbox.lock()?;
            ctx.store.update(id, |task| {
                if !current(task, recipient, &message) {
                    became_stale = true;
                    return Ok(());
                }
                if recipient
                    .state_mut(task)?
                    .effects_applied
                    .contains(&message.seq)
                {
                    return Ok(());
                }
                handler.apply_task(task, &message, &effect.receipt)?;
                match recipient {
                    Recipient::Run { run, .. } => {
                        let run = task
                            .runs
                            .iter_mut()
                            .find(|r| &r.id == run)
                            .context("effect run disappeared")?;
                        run.blocked_excerpt = None;
                        run.seen_working = false;
                        run.idle_since = None;
                        run.idle_no_report = false;
                        run.pending_reply = None;
                        if task.status == Status::NeedsMe {
                            task.status = Status::Running;
                        }
                    }
                    Recipient::PlanRun { plan_run, .. } => {
                        task.plan_runs
                            .iter_mut()
                            .find(|r| &r.id == plan_run)
                            .context("effect plan run disappeared")?
                            .pending_reply = None;
                    }
                    Recipient::PlanReview { plan, review, .. } => {
                        task.plans
                            .iter_mut()
                            .find(|p| &p.id == plan)
                            .context("effect plan disappeared")?
                            .reviews
                            .iter_mut()
                            .find(|r| &r.id == review)
                            .context("effect review disappeared")?
                            .pending_reply = None;
                    }
                    Recipient::Pm => {}
                }
                recipient
                    .state_mut(task)?
                    .effects_applied
                    .insert(message.seq);
                Ok(())
            })?;
        }
        if became_stale {
            inbox.start_effect(message.seq, true)?;
            continue;
        }
        inbox.finish_effect(message.seq)?;
    }
    Ok(())
}

fn current(task: &Task, recipient: &Recipient, message: &Message) -> bool {
    if message
        .binding
        .review_round
        .is_some_and(|round| round != task.rounds)
    {
        return false;
    }
    match recipient {
        Recipient::Run { run, .. } => task.runs.last().is_some_and(|r| {
            &r.id == run
                && r.outcome.is_none()
                && r.ended.is_none()
                && message.binding.run.as_ref().is_none_or(|id| id == run)
                && message.binding.decision.as_ref().is_none_or(|id| {
                    r.obligations
                        .decisions
                        .iter()
                        .any(|d| &d.request.id == id && d.pending())
                })
        }),
        Recipient::PlanRun { plan_run, .. } => task.plan_runs.last().is_some_and(|r| {
            &r.id == plan_run
                && r.holds_slot()
                && message.binding.run.as_ref().is_none_or(|id| id == plan_run)
        }),
        Recipient::PlanReview { plan, review, .. } => task.plans.last().is_some_and(|p| {
            &p.id == plan
                && p.reviews.last().is_some_and(|r| {
                    &r.id == review
                        && r.verdict.is_none()
                        && message.binding.run.as_ref().is_none_or(|id| id == review)
                })
        }),
        Recipient::Pm => true,
    }
}
