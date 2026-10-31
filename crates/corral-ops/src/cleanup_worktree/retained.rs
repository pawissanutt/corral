//! Authority for the owner's explicit approved-worktree removal while retaining its branch.
use crate::{Ctx, review::ApproveStrategy};
use anyhow::{Context, Result, ensure};
use corral_core::{RunHead, RunOutcome, Status, TaskHead, records::RecordArea};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const APPROVALS: RecordArea<Approval> = RecordArea::new("worktree-cleanup-approvals");

#[derive(Serialize, Deserialize)]
struct Approval {
    version: u8,
    task: String,
    run: String,
    branch: String,
    worktree: PathBuf,
    base: String,
    plan_target: Option<corral_core::RunPlanTarget>,
    sha: String,
    keep_branch_strategy: bool,
    remove_worktree: bool,
}

fn key(task: &str, run: &str) -> String {
    format!("{task}/{run}")
}

/// Joins the actual approval's terminal task mutation, before any cleanup effect.
/// Every approval replaces the previous intent, including explicit keeps and other strategies.
pub(crate) fn record(
    ctx: &Ctx,
    task: &TaskHead,
    run: &RunHead,
    sha: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
) -> Result<()> {
    APPROVALS.save(
        &ctx.store,
        &key(&task.id, &run.id),
        &Approval {
            version: 1,
            task: task.id.clone(),
            run: run.id.clone(),
            branch: run.branch.clone(),
            worktree: run.worktree.clone(),
            base: run.base.clone(),
            plan_target: run.plan_target.clone(),
            sha: sha.into(),
            keep_branch_strategy: strategy == ApproveStrategy::KeepBranch,
            remove_worktree,
        },
    )
}

/// A legacy approved event also fences missing/changed authority; it never grants permission.
pub(crate) fn requested(ctx: &Ctx, task: &str, run: &str) -> Result<bool> {
    let receipt = APPROVALS.load(&ctx.store, &key(task, run))?;
    if receipt.is_some_and(|receipt| receipt.keep_branch_strategy) {
        return Ok(true);
    }
    Ok(ctx
        .store
        .last_event_where(Some(task), &["approved"], Some(run), |_| true)?
        .is_some_and(|event| event.detail["strategy"] == "keep_branch"))
}

/// Called only after managed-root/alias checks. No receipt or changed authority fails closed.
pub(crate) fn require(ctx: &Ctx, task: &str, repo: &Path, run: &RunHead) -> Result<()> {
    let receipt = APPROVALS
        .load(&ctx.store, &key(task, &run.id))?
        .context("missing recorded retained-branch cleanup approval")?;
    ensure!(
        receipt.version == 1
            && receipt.task == task
            && receipt.run == run.id
            && receipt.branch == run.branch
            && receipt.worktree == run.worktree
            && receipt.base == run.base
            && receipt.plan_target == run.plan_target
            && receipt.keep_branch_strategy
            && receipt.remove_worktree,
        "recorded retained-branch cleanup approval changed"
    );
    let current = ctx.store.head(task)?;
    let actual = current
        .current_run
        .as_deref()
        .context("approved run disappeared")?;
    ensure!(
        current.status == Status::Done
            && actual.id == run.id
            && actual.outcome == Some(RunOutcome::Done)
            && actual.ended.is_some()
            && actual.branch == run.branch
            && actual.worktree == run.worktree
            && actual.base == run.base
            && actual.plan_target == run.plan_target,
        "approved retained-branch run changed"
    );
    let named = format!("refs/heads/{}", receipt.branch);
    ensure!(
        corral_git::is_ancestor(repo, &receipt.sha, &named)?,
        "retained named branch no longer preserves the approved commit"
    );
    ensure!(
        corral_git::rev_sha(&run.worktree, "HEAD")? == receipt.sha,
        "worktree HEAD differs from the approved commit"
    );
    Ok(())
}
