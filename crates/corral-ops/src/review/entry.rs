//! Approval entry points: who is asking decides whether the owner's override applies.
use anyhow::Result;

use super::{ApproveStrategy, approve_owner, pm_landing};
use crate::Ctx;

/// From Review, or Needs me after an autopilot escalation. FastForward/Rebase: `corral_git::land(project repo,
/// worktree, branch, base, ...)`; then, when `remove_worktree`, `worktree_remove`. Always releases the task's
/// owned herdr panes and empty workspaces. KeepBranch: no git changes, optional worktree removal. Status Done, logs
/// "approved" with the new sha. Deletes the merged branch after successful worktree removal unless `keep_branch`
/// is set. The review policy can disable worktree removal. Branch deletion failures are warnings only.
/// Returns a one-line human summary ("rebased corral/T-042 onto main at 1a2b3c4; worktree removed; branch deleted").
pub fn approve(
    ctx: &Ctx,
    id: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
    keep_branch: bool,
) -> Result<String> {
    approve_owner(ctx, id, strategy, remove_worktree, keep_branch, None, false)
}

/// `approve` for a command-line caller: the PM's shell lands only on the executor's evidence
/// bar, the owner's keeps the override. Owner surfaces (board, app) call `approve`.
pub fn approve_as(
    ctx: &Ctx,
    id: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
    keep_branch: bool,
    invoker: &crate::command::Invoker,
) -> Result<String> {
    let pm = pm_landing::caller(invoker);
    approve_owner(ctx, id, strategy, remove_worktree, keep_branch, None, pm)
}

/// Manual approval bound to the candidate the caller was shown. The authenticated app is the
/// owner; a PM invoker reaching this through the command registry keeps the PM bar.
pub fn approve_checked(
    ctx: &Ctx,
    id: &str,
    sha: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
    invoker: &crate::command::Invoker,
) -> Result<String> {
    let pm = pm_landing::caller(invoker);
    approve_owner(ctx, id, strategy, remove_worktree, false, Some(sha), pm)
}
