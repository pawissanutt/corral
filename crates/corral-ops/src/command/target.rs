//! Confirmation binds the server-observed target as well as the submitted payload.
use anyhow::Result;
use corral_core::Store;
use corral_protocol::cmd::{
    Command, plan::PlanCommand as Plan, review::ReviewCommand as Review, task::TaskCommand as Task,
};
use sha2::{Digest, Sha256};

/// Resolve only canonical task/run locations. No client path is opened. Daemon compares this
/// again in its queued worker; task revisions alone do not identify a run or stuck process.
pub fn app_target_fingerprint(store: &Store, command: &Command) -> Result<Option<String>> {
    let id = match command {
        Command::Task(
            Task::Retry { id, .. }
            | Task::Revert { id }
            | Task::Unstick { id }
            | Task::Move { id, .. }
            | Task::Idea { id, .. }
            | Task::WaiveReview { id, .. },
        )
        | Command::Plan(
            Plan::Accept { id, .. } | Plan::Cancel { id, .. } | Plan::Policy { id, .. },
        )
        | Command::Review(
            Review::Approve { id, .. }
            | Review::RequestChanges { id, .. }
            | Review::Verdict { id, .. }
            | Review::Discard { id },
        ) => id,
        _ => return Ok(None),
    };
    let head = store.head(id)?;
    let run = head.current_run.as_ref();
    let sha = run
        .filter(|run| run.worktree.is_dir())
        .map(|run| corral_git::rev_sha(&run.worktree, "HEAD"))
        .transpose()?;
    let bytes = serde_json::to_vec(&(head.rev, run.map(|run| (&run.id, &run.stuck)), sha))?;
    Ok(Some(format!("{:x}", Sha256::digest(bytes))))
}
