//! Recipient closure over typed task metadata, preserving native versus legacy rules.
use super::{Inbox, Recipient};
use crate::TaskRef;
use crate::agent::{ContainerRef, container_closure_ref};
use anyhow::Result;

impl Inbox<'_> {
    /// Bounded counterpart of `recipient_ended`. A failed metadata read remains an error,
    /// never evidence that a delivery may be retired.
    pub fn recipient_ended_ref<T: TaskRef + ?Sized>(&self, task: &T) -> Result<bool> {
        match &self.recipient {
            Recipient::Pm => Ok(false),
            Recipient::Assistant { session } => {
                Ok(self.store.get_assistant(session)?.ended.is_some())
            }
            Recipient::Run { task: id, run } => {
                let current = task.current_run_head()?;
                if current.as_ref().is_some_and(|record| {
                    record.agent_host.is_some()
                        || record.harness.as_deref().is_some_and(crate::is_pi_native)
                }) {
                    Ok(container_closure_ref(
                        task,
                        &ContainerRef::Worker {
                            task: id.clone(),
                            run: run.clone(),
                        },
                    )?
                    .is_some())
                } else {
                    Ok(current.is_none_or(|record| {
                        record.id != *run || record.outcome.is_some() || record.ended.is_some()
                    }))
                }
            }
            Recipient::ReviewRun {
                task: id,
                run,
                review,
            } => Ok(container_closure_ref(
                task,
                &ContainerRef::Reviewer {
                    task: id.clone(),
                    run: run.clone(),
                    review: review.clone(),
                },
            )?
            .is_some()),
            Recipient::PlanRun { task: id, plan_run } => {
                let current = task.last_plan_run_head()?;
                if current
                    .as_ref()
                    .is_some_and(|record| record.agent_host.is_some())
                {
                    Ok(container_closure_ref(
                        task,
                        &ContainerRef::Planner {
                            task: id.clone(),
                            run: plan_run.clone(),
                        },
                    )?
                    .is_some())
                } else {
                    Ok(current.is_none_or(|record| record.id != *plan_run || !record.holds_slot()))
                }
            }
            Recipient::PlanReview {
                task: id,
                plan,
                review,
            } => {
                let parent = task.plan_head(plan)?;
                let current = parent
                    .as_ref()
                    .and_then(|parent| parent.current_review.as_ref());
                if current.is_some_and(|record| record.agent_host.is_some()) {
                    Ok(container_closure_ref(
                        task,
                        &ContainerRef::PlanReviewer {
                            task: id.clone(),
                            plan: plan.clone(),
                            review: review.clone(),
                        },
                    )?
                    .is_some())
                } else {
                    Ok(current
                        .is_none_or(|record| record.id != *review || record.verdict.is_some()))
                }
            }
        }
    }
}
