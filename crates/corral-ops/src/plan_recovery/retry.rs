//! Preserve committed Plan/co-work candidates across board worker continuation.
use anyhow::{Result, ensure};
use corral_core::{
    Run, RunHead, RunOutcome, RunPlanTarget, Store, Task, TaskFieldsRef as _, TaskHead,
};
use serde::{Deserialize, Serialize};

const AREA: &str = "plan-run-resume";
#[cfg(test)]
#[path = "cowork_retry_tests.rs"]
mod tests;

#[derive(Serialize, Deserialize)]
pub(crate) struct RetrySource {
    source_run: String,
    head: String,
    base: String,
    #[serde(default)]
    target: Option<RunPlanTarget>,
}

/// Validate before superseding the source. Dirty work is never silently omitted from the
/// replacement checkout: the owner must preserve/commit it first, even when its pane is gone.
pub(crate) fn require_source(run: &Run, base: &str) -> Result<()> {
    ensure!(
        run.worktree.is_dir(),
        "Plan retry source worktree is missing; recover retained refs before retrying"
    );
    ensure!(
        corral_git::is_clean(&run.worktree)?,
        "Plan retry source is dirty; preserve and commit tracked/untracked work before retrying"
    );
    ensure!(
        corral_git::current_branch(&run.worktree)? == run.branch,
        "Plan retry source branch changed"
    );
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    ensure!(
        corral_git::is_ancestor(&run.worktree, base, &head)?,
        "Plan target advanced; reconcile the retained candidate before retrying"
    );
    Ok(())
}

/// Freeze the exact source in the same transaction that reserves the new run. Target
/// movement refuses explicitly rather than making a compatible checkpoint lie about HEAD.
pub(crate) fn source(task: &Task, run: &Run) -> Result<Option<RetrySource>> {
    let Some(previous) = task
        .current_run()
        .filter(|r| r.outcome == Some(RunOutcome::Superseded))
    else {
        return Ok(None);
    };
    let target = previous.plan_target.as_ref();
    let board_source = task
        .cowork_run()
        .is_some_and(|cowork| cowork.assistant_driver().is_none());
    if target.is_none() && !board_source {
        return Ok(None);
    }
    ensure!(
        run.plan_target.as_ref() == target,
        "Plan retry target changed; explicit reconciliation required"
    );
    require_source(previous, &run.base_sha)?;
    Ok(Some(RetrySource {
        source_run: previous.id.clone(),
        head: corral_git::rev_sha(&previous.worktree, "HEAD")?,
        base: run.base_sha.clone(),
        target: target.cloned(),
    }))
}

pub(crate) fn stage(store: &Store, task: &str, run: &str, source: &RetrySource) -> Result<()> {
    store.save_record(AREA, &format!("{task}-{run}"), source)
}

/// A reserved launch always uses its frozen source, never a mutable prior branch tip.
pub(crate) fn start_head(store: &Store, task: &TaskHead, run: &RunHead) -> Result<String> {
    let source = store.record::<RetrySource>(AREA, &format!("{}-{}", task.id, run.id))?;
    let Some(source) = source else {
        ensure!(
            (run.plan_target.is_none()
                && !task
                    .cowork_run()
                    .is_some_and(|cowork| cowork.assistant_driver().is_none()))
                || !task
                    .previous_run_head()?
                    .is_some_and(|previous| previous.outcome == Some(RunOutcome::Superseded)
                        && (previous.plan_target.is_some()
                            || task
                                .cowork_run()
                                .is_some_and(|cowork| cowork.assistant_driver().is_none()))),
            "Plan retry reservation lost its source; retain refs for explicit recovery"
        );
        return Ok(run.base_sha.clone());
    };
    ensure!(
        source.base == run.base_sha && run.plan_target.as_ref() == source.target.as_ref(),
        "Plan retry reservation changed"
    );
    ensure!(
        !source.source_run.is_empty(),
        "Plan retry source run missing"
    );
    Ok(source.head)
}
