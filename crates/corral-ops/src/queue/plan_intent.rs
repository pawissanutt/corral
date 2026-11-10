//! Current explicit planning intent, shared by queue and dispatch admission.
use anyhow::{Context, Result};
use corral_core::{Store, Task, TaskRef};

pub(crate) fn queued_plan_requested<T: TaskRef + ?Sized>(task: &T) -> bool {
    *task.queued()
        && task
            .dispatch_plan()
            .as_ref()
            .is_some_and(|plan| plan.explicit_plan)
}

/// Only the latest submission for the current task contract can hold admission.
pub(crate) fn pending_plan_reason(store: &Store, task: &Task) -> Result<Option<String>> {
    if task
        .cowork
        .as_ref()
        .is_some_and(|c| c.review == corral_core::CoworkReview::TransferV1)
        && let Some(journal) =
            crate::assistant::workflow::handoff::lifecycle::for_task(store, task)?
        && crate::assistant::workflow::handoff::plan_adoption::draft_pending(
            task,
            journal.transfer.as_ref().context("transfer missing")?,
        )?
    {
        // This projection is not authority. The executor still checks current driver,
        // exact durable instruction, named holds, capacity and dependencies.
        return Ok(None);
    }
    let Some(plan) = task.plans.last() else {
        return Ok(None);
    };
    if plan.spec_revision != crate::plan_spec_revision(task)? || plan.approval.is_some() {
        return Ok(None);
    }
    let waiting = match plan.reviews.last().and_then(|review| review.verdict) {
        None => "plan review verdict",
        Some(corral_core::Verdict::Approve | corral_core::Verdict::Escalate) => {
            "owner plan verdict"
        }
        Some(corral_core::Verdict::Changes) => return Ok(None),
    };
    Ok(Some(format!(
        "plan {} pending: awaiting {waiting}",
        plan.id
    )))
}
