use anyhow::{Context, Result, ensure};
use corral_core::{Run, Status, Store, Task};

use crate::{Ctx, review_budget::ReviewLaunch, verification};

pub(super) fn submit<R>(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    launch: &mut ReviewLaunch<'_>,
    pending_reply: Option<&str>,
    send: impl FnOnce() -> Result<R>,
) -> Result<R> {
    launch.submit(
        |store, current| {
            ensure!(
                current.id == task.id && current.status == Status::Review,
                "review task changed before submission"
            );
            let parent = current
                .current_run()
                .filter(|r| r.id == run.id)
                .context("review parent changed before submission")?;
            ensure!(
                parent
                    .review_runs
                    .last()
                    .and_then(|r| r.pending_reply.as_deref())
                    == pending_reply,
                "review transport request changed before submission"
            );
            current_assessment(ctx, store, current, parent)
        },
        send,
    )
}

/// Every reviewer submission, new or resumed, re-checks the candidate, the recorded route
/// and reviewer family against the fresh store under the same mutation boundary.
pub(crate) fn current_assessment(
    ctx: &Ctx,
    store: &Store,
    task: &Task,
    parent: &Run,
) -> Result<()> {
    let record = parent
        .review
        .as_ref()
        .context("review record disappeared")?;
    let mut fresh = ctx.clone();
    fresh.store = store.clone();
    ensure!(
        verification::review_changes(&fresh, task, parent, record)?.is_empty()
            && corral_git::is_clean(&parent.worktree)?,
        "review candidate or evidence changed before submission"
    );
    if let Some(route) = record.route.as_deref() {
        let changes =
            crate::review_packet::route_changes(&fresh, task, parent, record, route, false)?;
        ensure!(
            changes.is_empty(),
            "review assessment changed before submission: {}",
            serde_json::to_string(&changes)?
        );
    }
    let mut policy = store.config.task_review_policy(task);
    if crate::review_routing::choice(&store.config, task, None)
        == corral_core::Reviewer::CrossVendor
    {
        policy.require_cross_family = true;
    }
    let (_, resolved) = crate::review_routing::resolve(
        &store.config,
        &if record.reviewer == "pm" {
            corral_core::Reviewer::Pm
        } else {
            corral_core::Reviewer::Profile(record.reviewer.clone())
        },
        parent.vendor.as_deref(),
    )?;
    policy.check_reviewer_family(parent.vendor.as_deref(), resolved.vendor.as_deref())
}
