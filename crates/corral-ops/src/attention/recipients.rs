//! Active delivery metadata, keyed by container rather than assembled history vectors.
use anyhow::Result;
use corral_core::inbox::Recipient;
use corral_core::{InboxHead, TaskRef};

pub(super) fn recipients(task: &(impl TaskRef + ?Sized)) -> Result<Vec<(Recipient, InboxHead)>> {
    let mut recipients = Vec::new();
    if let Some(run) = task.current_run_head()? {
        recipients.push((
            Recipient::Run {
                task: task.id().clone(),
                run: run.id.clone(),
            },
            run.inbox.clone(),
        ));
        for review in task.open_review_children(&run.id)? {
            if review.outcome.is_none() && review.ended.is_none() {
                recipients.push((
                    Recipient::ReviewRun {
                        task: task.id().clone(),
                        run: run.id.clone(),
                        review: review.id.clone(),
                    },
                    review.inbox,
                ));
            }
        }
    }
    if let Some(run) = task.last_plan_run_head()?.filter(|run| run.holds_slot()) {
        recipients.push((
            Recipient::PlanRun {
                task: task.id().clone(),
                plan_run: run.id.clone(),
            },
            run.inbox.clone(),
        ));
    }
    for (plan, review) in task.pending_plan_reviewers()? {
        recipients.push((
            Recipient::PlanReview {
                task: task.id().clone(),
                plan,
                review: review.id,
            },
            review.inbox,
        ));
    }
    Ok(recipients)
}
