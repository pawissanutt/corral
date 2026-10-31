//! Finished-reviewer shutdown: verdict state and its event must both be durable before signals.
use crate::Ctx;
use anyhow::Result;
use corral_core::TaskRef;

pub(crate) fn reap_closed_reviews<T: TaskRef + ?Sized>(ctx: &Ctx, source: &T) {
    let result: Result<()> = (|| {
        // Read the actual complete task for this historical cleanup, never a projected Task
        // with an invented child vector. The owning-sync selection remains an audit seam.
        let task = ctx.store.get(source.id())?;
        if !task
            .runs
            .iter()
            .any(|run| run.review_runs.iter().any(|child| child.ended.is_some()))
        {
            return Ok(());
        }
        let events = ctx.store.events_of(Some(&task.id), &["verdict"])?;
        for run in &task.runs {
            for child in run.review_runs.iter().filter(|child| child.ended.is_some()) {
                if let Some(review) = child
                    .review
                    .as_ref()
                    .filter(|review| review.verdict.is_some())
                    && !events.iter().any(|event| {
                        event.kind == "verdict"
                            && event.detail["run"].as_str() == Some(&run.id)
                            && event.detail["reviewer"].as_str() == Some(&review.reviewer)
                            && event.ts >= review.at
                    })
                {
                    continue;
                }
                if let Err(error) = crate::cleanup_worktree::reap_review(ctx, &task, run, child) {
                    super::failed(
                        ctx,
                        &task.id,
                        &format!("review {}-{}", run.id, child.id),
                        &error,
                    );
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        super::failed(ctx, source.id(), "review event inventory", &error);
    }
}
