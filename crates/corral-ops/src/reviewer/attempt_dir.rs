//! Where a review attempt keeps its prompt, packet and notes: a registered directory under the
//! scratch root, keyed by the review's callback attempt.
use std::path::PathBuf;

use anyhow::Result;
use corral_core::ReviewRecord;

use crate::Ctx;

pub(super) const NOTES: &str = "notes.md";

/// Where review child `child` of run `run` keeps its prompt, packet and notes: its registered
/// scratch directory, outside the worktree, so writing there never dirties it.
pub(crate) fn review_dir(ctx: &Ctx, task: &str, run: &str, child: &str) -> Result<PathBuf> {
    crate::scratch_reclaim::Attempt::Review { task, run, child }.dir(ctx)
}

/// The attempt whose scratch directory a review uses: its callback attempt, which a retry of an
/// undelivered request inherits, so the retained request keeps naming the same files.
pub(crate) fn dir_attempt(review: Option<&ReviewRecord>, child: &str) -> String {
    review
        .and_then(|review| review.route.as_deref())
        .and_then(|route| route.callback.clone().or_else(|| route.attempt.clone()))
        .unwrap_or_else(|| child.to_owned())
}

/// The notes file of `run`'s current review, or of its first attempt before any launched.
pub(super) fn notes_of(ctx: &Ctx, task: &str, run: &corral_core::RunHead) -> Result<PathBuf> {
    let child = run
        .current_child
        .as_deref()
        .map_or_else(|| format!("{}-review1", run.id), |child| child.id.clone());
    let review = run
        .review
        .as_ref()
        .or_else(|| run.current_child.as_deref()?.review.as_ref());
    Ok(review_dir(ctx, task, &run.id, &dir_attempt(review, &child))?.join(NOTES))
}

/// Where the reviewer of `run`'s current review attempt (its first, before any) writes its notes.
pub fn review_notes_path(ctx: &Ctx, task: &str, run: &str) -> Result<PathBuf> {
    let head = ctx.store.head(task)?;
    let current = head
        .current_run
        .as_ref()
        .filter(|current| current.id == run);
    let latest = current.and_then(|current| current.current_child.as_deref());
    let child = latest.map_or_else(|| format!("{run}-review1"), |child| child.id.clone());
    let review = current
        .and_then(|current| current.review.as_ref())
        .or_else(|| latest?.review.as_ref());
    Ok(review_dir(ctx, task, run, &dir_attempt(review, &child))?.join(NOTES))
}
