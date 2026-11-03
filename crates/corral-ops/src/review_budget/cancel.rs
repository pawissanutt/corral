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
        let Some(current) = task.current_run_mut().filter(|r| r.id == run.id) else {
            return Ok(());
        };
        if review.is_some() && current.review.as_ref() == review {
            let mut record = current.review.take();
            if let Some(review) = record.as_mut() {
                review.cancelled = true;
            }
            if let Some(child) = current.current_child.as_deref_mut() {
                child.review = record;
                child.outcome = Some(RunOutcome::Cancelled);
                child.ended = Some(ctx.store.now());
                child.usage_pending = true;
                child.pending_reply = None;
            }
            ctx.store.log_event(
                Some(id),
                "review_cancelled",
                json!({"run": run.id, "result":"cancelled", "reason": reason}),
            )?;
        }
        Ok(())
    })?;
    if let Some(child) = run.current_child.as_deref().filter(|_| review.is_some()) {
        crate::cleanup_worktree::reap_review(ctx, id, &run.id, &child.id)?;
    }
    Ok(())
}

fn cancel_plan_reviews(ctx: &Ctx, id: &str, reason: &str) -> Result<()> {
    let head = ctx.store.head(id)?;
    for (plan, review) in head.pending_plan_reviewers()? {
        if review.reviewer != "pm"
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
        ctx.store.update_head(id, |task| {
            task.edit_plan_review(&plan, &review.id, |current| {
                if current.verdict.is_none() && !current.cancelled {
                    current.cancelled = true;
                    current.pending_reply = None;
                    ctx.store.log_event(Some(id), "review_cancelled", json!({"target":"plan", "plan":plan,"attempt":review.id,"result":"cancelled","reason":reason}))?;
                }
                Ok(())
            })
        })?;
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
