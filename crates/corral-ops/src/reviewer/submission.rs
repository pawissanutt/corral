use anyhow::{Context, Result, ensure};
use corral_core::{Run, Status, Task};

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
            let record = parent
                .review
                .as_ref()
                .context("review record disappeared")?;
            ensure!(
                parent
                    .review_runs
                    .last()
                    .and_then(|r| r.pending_reply.as_deref())
                    == pending_reply,
                "review transport request changed before submission"
            );
            let mut fresh = ctx.clone();
            fresh.store = store.clone();
            ensure!(
                verification::review_changes(&fresh, current, parent, record)?.is_empty()
                    && corral_git::is_clean(&parent.worktree)?,
                "review candidate or evidence changed before submission"
            );
            let mut policy = store.config.task_review_policy(current);
            if crate::review_routing::choice(&store.config, current, None)
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
            policy.check_reviewer_family(parent.vendor.as_deref(), resolved.vendor.as_deref())?;
            Ok(())
        },
        send,
    )
}
