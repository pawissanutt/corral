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
