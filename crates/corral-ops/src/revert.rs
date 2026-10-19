//! Owner-requested, exact-range reversal of a landed task.
use anyhow::{Context, Result, bail};
use corral_core::{Event, RunOutcome, Status, Task};
use corral_git::{RevertRange, RevertedRange};
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;

/// The reopened task and the exact history appended to undo its latest landing.
#[derive(Debug, Clone)]
pub struct RevertResult {
    pub task: Task,
    /// Original landed commits, newest first.
    pub original_shas: Vec<String>,
    /// New commits appended to the base, oldest first.
    pub revert_shas: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Strategy {
    FastForward,
    Rebase,
    KeepBranch,
}

#[derive(Deserialize)]
struct Approval {
    run: String,
    strategy: Strategy,
    sha: String,
    #[serde(alias = "base_before")]
    previous_sha: Option<String>,
    base: String,
    project: Option<String>,
}

fn landing(task: &Task, events: &[Event]) -> Result<RevertRange> {
    if task.status != Status::Done {
        bail!("{} is not a landed Done task", task.id);
    }
    let event = events
        .iter()
        .rev()
        .find(|e| {
            e.task.as_deref() == Some(&task.id)
                && matches!(e.kind.as_str(), "approved" | "reverted")
        })
        .with_context(|| format!("{} has no durable landing approval", task.id))?;
    if event.kind == "reverted" {
        bail!(
            "{} has already been reverted; a new landing is required",
            task.id
        );
    }
    let approval: Approval = serde_json::from_value(event.detail.clone())
        .context("approval lacks exact landing provenance; refusing to infer task commits")?;
    match approval.strategy {
        Strategy::FastForward | Strategy::Rebase => {}
        Strategy::KeepBranch => bail!("{} was kept on its branch, not landed", task.id),
    }
    let run = task.current_run().context("landed task has no run")?;
    if run.id != approval.run
        || run.base != approval.base
        || task.project != approval.project
        || run.outcome != Some(RunOutcome::Done)
    {
        bail!("{} no longer matches its durable landing approval", task.id);
    }
    Ok(RevertRange {
        base_before: approval.previous_sha.context(
            "approval has no base_before/previous_sha; refusing to use the dispatch base",
        )?,
        landed_sha: approval.sha,
    })
}

/// Match acceptance's peer-review exclusion while the board and this task are locked.
fn lock_peers(ctx: &Ctx, task: &Task) -> Result<Vec<corral_core::StoreLock>> {
    let mut locks = Vec::new();
    for peer in ctx.store.list()? {
        if peer.id == task.id || peer.project != task.project {
            continue;
        }
        let lock = ctx.store.try_lock(&peer.id)?.with_context(|| {
            format!("{} is changing; retry reverting after it settles", peer.id)
        })?;
        let peer = ctx.store.get(&peer.id)?;
        if peer.project == task.project
            && peer.status == Status::Review
            && let Some(run) = peer.current_run()
            && !run.review_interrupted()
            && let Some(review) = run.review.as_ref().filter(|r| r.verdict.is_none())
            && corral_git::rev_sha(&run.worktree, "HEAD")? == review.sha
        {
            bail!(
                "{} has a review in progress; wait for its verdict before reverting",
                peer.id
            );
        }
        locks.push(lock);
    }
    Ok(locks)
}

/// Revert the latest landed task and reopen it in Backlog with automatic dispatch held.
///
/// The durable approval's pre-landing base, never the dispatch base, identifies the originals.
/// Repository landing, board/config and task locks exclude concurrent acceptance and task
/// mutation. Pending post-land operations block advancement; the inverse gets the same
/// durable hook preparation and completion as any other base advancement.
///
/// # Errors
/// Refuses tasks without an exact, unreverted landing, concurrent task/review changes,
/// operational blockers, dirty/wrong base checkouts and conflicting later work. Git conflicts
/// report the full later SHAs and paths without changing the base or task.
pub fn revert(ctx: &Ctx, id: &str) -> Result<RevertResult> {
    ctx.store.check_runtime()?;
    let initial = ctx.store.get(id)?;
    let _autopilot = ctx
        .store
        .try_lock(&format!("autopilot-{}", initial.id))?
        .with_context(|| format!("approval or revert already in progress for {}", initial.id))?;
    let run = initial.current_run().context("task has no landed run")?;
    let repo = crate::review::main_repo(ctx, &initial, run)?;
    let _landing = corral_git::landing_lock(&repo)?;
    crate::hooks::reconcile_locked(ctx, &repo)?;

    let (task, (reverted, mut operation, range, run_id)) =
        ctx.store.dispatch_update(&initial.id, |store, task| {
            let mut fresh = ctx.clone();
            fresh.store = store.clone();
            let range = landing(task, store.events_shared()?.as_slice())?;
            let run = crate::review::current_run(task)?;
            if crate::review::main_repo(&fresh, task, &run)? != repo {
                bail!("task project changed while acquiring landing lock; retry reverting");
            }
            if let Some(project) = task.project.as_deref()
                && let Some(reason) = crate::hooks::operational_blocker(&fresh, project)?
            {
                bail!("{reason}; resolve post-land operations before reverting");
            }
            let _peers = lock_peers(&fresh, task)?;
            let mut operation = None;
            let reverted: RevertedRange =
                corral_git::revert_range_prepared(&repo, &run.base, &range, |before, sha| {
                    operation = crate::hooks::prepare(&fresh, task, &run, &repo, before, sha)?;
                    Ok(())
                })?;
            task.status = Status::Backlog;
            task.dispatch_hold = true;
            task.queued = false;
            task.dispatch_plan = None;
            task.launch = None;
            task.review_reservation = None;
            task.review_wait = None;
            if let Some(run) = task.current_run_mut() {
                run.autopilot = None;
                run.retry = None;
                run.pending_reply = None;
            }
            Ok((reverted, operation, range, run.id))
        })?;

    ctx.store
        .log_event(
            Some(&task.id),
            "reverted",
            json!({
                "run": run_id, "project": task.project,
                "base": task.current_run().map(|r| &r.base),
                "base_before": reverted.base_before,
                "landing_base_before": range.base_before,
                "landed_sha": range.landed_sha,
                "original_shas": reverted.original_shas,
                "revert_shas": reverted.revert_shas,
                "sha": reverted.revert_shas.last(),
                "from": "done", "to": "backlog", "dispatch_hold": true,
            }),
        )
        .with_context(|| {
            format!(
                "{} reverted at {}; task reopened, but recording the reverted event failed",
                task.id,
                reverted.revert_shas.join(", ")
            )
        })?;
    if let Some(op) = &mut operation
        && let Err(error) = crate::hooks::complete_landing(ctx, op)
    {
        // Base and reopened task are already durable, as for successful acceptance.
        crate::messages::report(
            Some(&task.id),
            "post-land",
            format!("reverted; operation recovery needed: {error:#}"),
        );
    }
    Ok(RevertResult {
        task,
        original_shas: reverted.original_shas,
        revert_shas: reverted.revert_shas,
    })
}

#[cfg(test)]
#[path = "revert_tests.rs"]
mod tests;
