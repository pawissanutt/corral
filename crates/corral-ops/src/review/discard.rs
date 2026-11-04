//! Explicit task/run disposal, including recoverable co-work scratch preservation.
use anyhow::{Result, bail};
use corral_core::{RunOutcome, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use super::main_repo;
use crate::Ctx;
use crate::cleanup::Cleanup;

/// Discard the current run and remove its worktree and branch. Artifacts remain available.
pub fn discard(ctx: &Ctx, id: &str) -> Result<Task> {
    discard_with_options(ctx, id, false)
}

/// `keep_branch` retains the discarded branch, but still removes the worktree.
pub fn discard_with_options(ctx: &Ctx, id: &str, keep_branch: bool) -> Result<Task> {
    discard_with_decision(ctx, id, keep_branch, None, "owner")
}

/// Discard with the supplied rationale and actual decision author, independently of consent.
pub fn discard_with_decision(
    ctx: &Ctx,
    id: &str,
    keep_branch: bool,
    reason: Option<&str>,
    actor: &str,
) -> Result<Task> {
    let task = ctx.store.update(id, |t| {
        if let Some(r) = t.current_run_mut() {
            r.outcome = Some(RunOutcome::Discarded);
            r.ended.get_or_insert_with(Timestamp::now);
        }
        t.status = Status::Dropped;
        t.queued = false;
        Ok(())
    })?;
    let mut warnings = crate::cleanup::cleanup_report(ctx, &task, Cleanup::Task);
    let mut removed = false;
    let mut branch_deleted = false;
    if let Some(run) = task.current_run() {
        let result = (|| -> Result<()> {
            let repo = main_repo(ctx, &task, &corral_core::RunHead::of(run)?)?;
            if !warnings.is_empty() {
                bail!("panes could not be closed");
            }
            let _scratch = crate::assistant::scratch::preserve_discarded(ctx, &task)?;
            crate::cleanup_worktree::remove(
                ctx,
                &task,
                &repo,
                &corral_core::RunHead::of(run)?,
                true,
            )?;
            removed = true;
            if !keep_branch {
                crate::cleanup_worktree::discard_branch(&repo, run)?;
                branch_deleted = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            warnings.push(format!("{error:#}"));
        }
    }
    for warning in &warnings {
        crate::messages::report(
            Some(&task.id),
            "discard",
            format!("warning: discard {}: {warning}", task.id),
        );
    }
    ctx.store.log_event(
        Some(&task.id),
        "discarded",
        json!({ "run": task.current_run().map(|r| r.id.clone()), "worktree_removed": removed,
            "branch_deleted": branch_deleted, "keep_branch": keep_branch, "warnings": warnings,
            "task_updated": task.updated, "reason": reason, "actor": actor }),
    )?;
    crate::assistant::scratch::release_discarded(ctx, &task);
    Ok(task)
}
