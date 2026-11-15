//! Called with the review-budget lease, so cancellation cannot race a new launch.
use anyhow::Result;
use corral_core::{ReviewWork, RunOutcome};
use serde_json::json;

use crate::Ctx;

pub(crate) fn cancel_task(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    let head = ctx.store.head(id)?;
    if !crate::session_scope::owns_head(ctx, &head)? {
        return Ok(());
    }
    // Recover a launcher which died while preparing. Never retain a cancelled preparation.
    if let Some(pending) = super::load_pending(ctx)?.filter(|p| p.task == id) {
        if pending.close_pane
            && let Some(pane) = &pending.pane
            && ctx.herdr.pane_get(pane)?.is_some()
        {
            ctx.herdr.pane_close(pane)?;
        }
        super::clear(ctx)?;
        super::submission::clear_retained(ctx, id, &pending.run, pending.kind)?;
    }
    if let Some(run) = head.current_run.as_deref() {
        for kind in [ReviewWork::Gate, ReviewWork::Reviewer] {
            if let Some(prepared) = super::submission::load_retained(ctx, id, &run.id, kind)? {
                ctx.store.update_head(id, |task| {
                    if let Some(current) = task.current_run_mut().filter(|r| r.id == run.id) {
                        if current.gate.is_none() {
                            current.gate = prepared.prepared_gate.clone();
                        }
                        if current.review.is_none() && prepared.prepared_review.is_some() {
                            current.review = prepared.prepared_review.clone();
                            if let Some(child) = current
                                .current_child
                                .as_deref_mut()
                                .filter(|c| Some(&c.id) == prepared.review_run.as_ref())
                            {
                                child.ended = None;
                            }
                        }
                    }
                    super::submission::clear_retained(ctx, id, &run.id, kind)?;
                    ctx.store.log_event(
                        Some(id),
                        "review_launch_cancelled",
                        json!({"run":run.id,"kind":kind,"result":"cancelled","reason":reason}),
                    )
                })?;
            }
        }
    }
    cancel_plan_reviews(ctx, id, reason)?;
    crate::gate::cancel(ctx, id, reason)?;
    crate::plan_acceptance::cancel_gate(ctx, id, reason)?;
    crate::plan_acceptance::checkpoint::cancel(ctx, id, reason)?;
    let head = ctx.store.head(id)?;
    let Some(run) = head.current_run.as_deref() else {
        ctx.store.update_head(id, |task| {
            task.review_reservation = None;
            task.review_wait = None;
            Ok(())
        })?;
        return Ok(());
    };
    let review = run.review.as_ref().filter(|r| r.holds_slot());
    if let Some(review) = review
        && review.reviewer != "pm"
        && let Some(pane) = review.pane.as_deref()
        && run.herdr.as_ref().is_none_or(|h| h.pane_id != pane)
        && ctx.herdr.pane_get(pane)?.is_some()
    {
        ctx.herdr.pane_close(pane)?;
    }
    ctx.store.update_head(id, |task| {
        task.review_reservation = None;
        task.review_wait = None;
        Ok(())
    })?;
    if let Some(child) = run.current_child.as_deref().filter(|_| review.is_some()) {
        cancel_reviewer(ctx, id, &run.id, &child.id, reason)?;
    }
    if review.is_some() && run.current_child.is_none() {
        ctx.store.update_head(id, |task| {
            if let Some(current) = task
                .current_run_mut()
                .filter(|current| current.id == run.id)
            {
                current.review = None;
            }
            Ok(())
        })?;
    }
    if let Some(child) = run.current_child.as_deref().filter(|_| review.is_some()) {
        crate::cleanup_worktree::reap_review(ctx, id, &run.id, &child.id)?;
    }
    Ok(())
}

/// Record-only half of review cancellation. Terminate stops the native host first
/// and closes its exact pane afterwards, without cancelling its parent or siblings.
pub(crate) fn cancel_reviewer(
    ctx: &Ctx,
    id: &str,
    run: &str,
    child: &str,
    reason: &str,
) -> Result<()> {
    ctx.store.update_head(id, |task| {
        let current = task
            .current_run_mut()
            .filter(|current| current.id == run)
            .ok_or_else(|| anyhow::anyhow!("review parent changed"))?;
        let latest = current
            .current_child
            .as_deref()
            .is_some_and(|current| current.id == child);
        if latest {
            if current
                .current_child
                .as_deref()
                .is_some_and(|child| child.ended.is_some())
            {
                return Ok(());
            }
            let mut record = current.review.take();
            if let Some(review) = record.as_mut() {
                review.cancelled = true;
                review.verdict = None;
            }
            if let Some(cancelled) = current.current_child.as_deref_mut() {
                cancelled.review = record;
                cancelled.outcome = Some(RunOutcome::Cancelled);
                cancelled.ended = Some(ctx.store.now());
                cancelled.usage_pending = true;
                cancelled.pending_reply = None;
            }
            task.review_reservation = None;
            task.review_wait = None;
        } else {
            task.edit_review_child(run, child, |cancelled| {
                if let Some(review) = cancelled.review.as_mut() {
                    review.cancelled = true;
                    review.verdict = None;
                }
                cancelled.outcome = Some(RunOutcome::Cancelled);
                cancelled.ended.get_or_insert(ctx.store.now());
                cancelled.usage_pending = true;
                cancelled.pending_reply = None;
                Ok(())
            })?;
        }
        ctx.store.log_event(
            Some(id),
            "review_cancelled",
            json!({"run":run,"review":child,"result":"cancelled","reason":reason}),
        )
    })?;
    Ok(())
}

fn cancel_plan_reviews(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    let head = ctx.store.head(id)?;
    for (plan, review) in head.pending_plan_reviewers()? {
        if crate::plan_review::context::can_close(ctx, &review)?
            && let Some(herdr) = &review.herdr
            && head
                .last_plan_run
                .as_ref()
                .and_then(|r| r.herdr.as_ref())
                .is_none_or(|h| h.pane_id != herdr.pane_id)
            && ctx.herdr.pane_get(&herdr.pane_id)?.is_some()
        {
            ctx.herdr.pane_close(&herdr.pane_id)?;
        }
        crate::plan_review::cancel_attempt(ctx, id, &plan, &review.id, reason)?;
    }
    if let Some(attempt) = head.last_aggregate_attempt.as_deref()
        && let Some(review) = attempt.current_review.as_ref().filter(|r| r.pending())
    {
        if review.reviewer != "pm"
            && let Some(herdr) = &review.herdr
            && ctx.herdr.pane_get(&herdr.pane_id)?.is_some()
        {
            ctx.herdr.pane_close(&herdr.pane_id)?;
        }
        ctx.store.update_head(id, |task| {
            task.edit_aggregate_review(&attempt.id, &review.id, |current| {
                if current.pending() {
                    current.abandoned = Some(format!("cancelled: {reason}"));
                    ctx.store.log_event(Some(id), "review_cancelled", json!({"target":"aggregate", "attempt":review.id,"result":"cancelled","reason":reason}))?;
                }
                Ok(())
            })
        })?;
    }
    Ok(())
}
