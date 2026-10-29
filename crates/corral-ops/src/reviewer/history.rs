//! Preserve each review's accounting and verdict when it ends or is superseded.
use corral_core::{Run, RunOutcome};
use jiff::Timestamp;

use crate::Ctx;

pub(super) fn finish_review(ctx: &Ctx, parent: &mut Run, ended: Timestamp) {
    if let Some(snapshot) = parent.review_runs.last_mut() {
        if let Some(record) = &parent.review {
            let mut record = record.clone();
            if record.notes.is_none() {
                record.notes = snapshot.review.as_ref().and_then(|r| r.notes.clone());
            }
            snapshot.review = Some(record);
        }
        if snapshot.ended.is_none() {
            snapshot.ended = Some(ended);
            crate::usage::collect(snapshot, &ctx.store);
        }
    }
}

/// [`finish_review`] on a head's current run: the snapshot is its current child, whose usage
/// [`collect_usage`] collects; no earlier child is read.
pub(super) fn finish_review_head(parent: &mut corral_core::RunHead, ended: Timestamp) {
    let record = parent.review.clone();
    if let Some(snapshot) = parent.current_child.as_deref_mut() {
        if let Some(mut record) = record {
            if record.notes.is_none() {
                record.notes = snapshot.review.as_ref().and_then(|r| r.notes.clone());
            }
            snapshot.review = Some(record);
        }
        if snapshot.ended.is_none() {
            snapshot.ended = Some(ended);
            snapshot.usage_pending = true;
        }
    }
}

/// End a pending review without inventing a verdict for a superseded candidate.
pub(crate) fn supersede_review(ctx: &Ctx, parent: &mut Run, ended: Timestamp) {
    if parent.review.as_ref().is_none_or(|r| r.verdict.is_some()) {
        return;
    }
    finish_review(ctx, parent, ended);
    parent.review = None;
    if let Some(child) = parent.review_runs.last_mut() {
        child.outcome = Some(RunOutcome::Superseded);
        child.pending_reply = None;
    }
    parent.idle_since = None;
    parent.blocked_excerpt = None;
}

/// [`supersede_review`] on a head's current run: the review snapshot is its current child,
/// whose usage the next usage refresh collects (`usage_pending`), as
/// `review_budget::finish_head` leaves it; no earlier child is read.
pub(crate) fn supersede_review_head(
    task: &mut corral_core::TaskHeadMut,
    ended: Timestamp,
) -> anyhow::Result<()> {
    let Some(parent) = task.current_run_mut() else {
        return Ok(());
    };
    if parent.review.as_ref().is_none_or(|r| r.verdict.is_some()) {
        return Ok(());
    }
    let record = parent.review.take();
    if let Some(snapshot) = parent.current_child.as_deref_mut() {
        if let Some(mut record) = record {
            if record.notes.is_none() {
                record.notes = snapshot.review.as_ref().and_then(|r| r.notes.clone());
            }
            snapshot.review = Some(record);
        }
        if snapshot.ended.is_none() {
            snapshot.ended = Some(ended);
            snapshot.usage_pending = true;
        }
        snapshot.outcome = Some(RunOutcome::Superseded);
        snapshot.pending_reply = None;
    }
    parent.idle_since = None;
    parent.blocked_excerpt = None;
    Ok(())
}
