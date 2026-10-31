//! Semantic container closure from current metadata and a named child, without host history.
use super::{ClosureReason, ContainerRef};
use crate::{RunOutcome, Status, TaskRef};
use anyhow::Result;

/// The bounded counterpart of `container_closure`; IO failures are not closure evidence.
pub fn container_closure_ref<T: TaskRef + ?Sized>(
    task: &T,
    container: &ContainerRef,
) -> Result<Option<ClosureReason>> {
    let closed = |reason| Ok(Some(reason));
    match container {
        ContainerRef::Pm | ContainerRef::Assistant { .. } => Ok(None),
        ContainerRef::Planner { task: id, run } => {
            if task.id() != id || *task.status() == Status::Dropped {
                return closed(ClosureReason::RunEnded);
            }
            Ok(task
                .last_plan_run_head()?
                .is_none_or(|current| current.id != *run || !current.holds_slot())
                .then_some(ClosureReason::RunEnded))
        }
        ContainerRef::PlanReviewer {
            task: id,
            plan,
            review,
        } => {
            if task.id() != id || *task.status() == Status::Dropped {
                return closed(ClosureReason::RunEnded);
            }
            let Some(parent) = task.last_plan_head()?.filter(|parent| parent.id == *plan) else {
                return closed(ClosureReason::ReviewSuperseded);
            };
            let latest = task.last_plan_run_head()?;
            if !crate::plan_contract_revision(task)
                .is_ok_and(|revision| revision == parent.spec_revision)
                || parent
                    .run
                    .as_ref()
                    .is_some_and(|run| latest.as_ref().is_none_or(|current| current.id != *run))
            {
                return closed(ClosureReason::ReviewSuperseded);
            }
            let Some(child) = parent
                .current_review
                .as_ref()
                .filter(|child| child.id == *review)
            else {
                return closed(ClosureReason::ReviewSuperseded);
            };
            Ok(child
                .verdict
                .is_some()
                .then_some(ClosureReason::ReviewClosed))
        }
        ContainerRef::Worker { task: id, run } | ContainerRef::Reviewer { task: id, run, .. } => {
            if task.id() != id || *task.status() == Status::Dropped {
                return closed(ClosureReason::RunEnded);
            }
            let Some(parent) = task.current_run_head()?.filter(|parent| parent.id == *run) else {
                return closed(ClosureReason::RunEnded);
            };
            if matches!(
                parent.outcome,
                Some(RunOutcome::Discarded | RunOutcome::Superseded | RunOutcome::Abandoned)
            ) || (parent.outcome == Some(RunOutcome::Done) && *task.status() == Status::Done)
            {
                return closed(ClosureReason::RunEnded);
            }
            let ContainerRef::Reviewer { review, .. } = container else {
                return Ok(None);
            };
            let current = parent
                .current_child
                .as_deref()
                .filter(|child| child.id == *review);
            let last = current.is_some();
            let child = match current {
                Some(child) => Some(child.clone()),
                None => task.review_child(run, review)?,
            };
            let Some(child) = child else {
                return closed(ClosureReason::ReviewSuperseded);
            };
            let active = parent.review.as_ref().filter(|_| last);
            let records = || child.review.iter().chain(active);
            Ok(if records().any(|record| record.verdict.is_some()) {
                Some(ClosureReason::ReviewClosed)
            } else if child.outcome == Some(RunOutcome::Superseded) {
                Some(ClosureReason::ReviewSuperseded)
            } else if child.outcome.is_some() || records().any(|record| record.process_lost) {
                Some(ClosureReason::ReviewInterrupted)
            } else if !last {
                Some(ClosureReason::ReviewSuperseded)
            } else {
                None
            })
        }
    }
}
