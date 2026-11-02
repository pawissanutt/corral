//! Typed delivery inputs; only metadata of the current containers and last pending reviewers.
use super::{Condition, Delivery, Neighborhood, Recipient, Timestamp, project};
use anyhow::Result;
use corral_core::agent::{ContainerRef, container_closure_ref};
use corral_core::{RunHead, TaskRef};

fn run_delivery(
    recipient: Recipient,
    run: &RunHead,
    task_reply: bool,
    neighborhood: &Neighborhood<'_>,
) -> Delivery {
    Delivery {
        recipient,
        state: run.inbox.clone(),
        mode: run.delivery,
        pending: run.pending_reply.clone(),
        unacted: run.idle_no_report && run.outcome.is_none() && run.ended.is_none(),
        orphaned: neighborhood.inventory_known && run.agent_state.as_deref() == Some("gone"),
        task_reply: task_reply && run.outcome.is_none() && run.ended.is_none(),
    }
}

pub(in crate::attention) fn conditions_ref(
    task: &(impl TaskRef + ?Sized),
    neighborhood: &Neighborhood<'_>,
    now: Timestamp,
) -> Result<Vec<Condition>> {
    let mut recipients = Vec::new();
    if let Some(run) = task.current_run_head()? {
        let container = ContainerRef::Worker {
            task: task.id().clone(),
            run: run.id.clone(),
        };
        let native = run.agent_host.is_some()
            || run
                .harness
                .as_deref()
                .is_some_and(corral_core::is_corral_pi);
        let open = if native {
            container_closure_ref(task, &container)?.is_none()
        } else {
            run.outcome.is_none() && run.ended.is_none()
        };
        if open {
            recipients.push(run_delivery(
                Recipient::Run {
                    task: task.id().clone(),
                    run: run.id.clone(),
                },
                &run,
                true,
                neighborhood,
            ));
        }
        // Non-last children are semantically superseded/interrupted. The last one is
        // retained even if ended alone: closure is NOT synonymous with that timestamp.
        if let Some(review) = run.current_child.as_deref() {
            let container = ContainerRef::Reviewer {
                task: task.id().clone(),
                run: run.id.clone(),
                review: review.id.clone(),
            };
            if container_closure_ref(task, &container)?.is_none() {
                recipients.push(run_delivery(
                    Recipient::ReviewRun {
                        task: task.id().clone(),
                        run: run.id.clone(),
                        review: review.id.clone(),
                    },
                    review,
                    false,
                    neighborhood,
                ));
            }
        }
    }
    if let Some(run) = task.last_plan_run_head()?.filter(|run| run.holds_slot()) {
        recipients.push(Delivery {
            recipient: Recipient::PlanRun {
                task: task.id().clone(),
                plan_run: run.id.clone(),
            },
            state: run.inbox.clone(),
            mode: run.delivery,
            pending: run.pending_reply.clone(),
            unacted: true,
            orphaned: false,
            task_reply: true,
        });
    }
    for (plan, review) in task.pending_plan_reviewers()? {
        recipients.push(Delivery {
            recipient: Recipient::PlanReview {
                task: task.id().clone(),
                plan,
                review: review.id,
            },
            state: review.inbox,
            mode: review.delivery,
            pending: review.pending_reply,
            unacted: true,
            orphaned: false,
            task_reply: false,
        });
    }
    let mut out = Vec::new();
    project(task, neighborhood, now, recipients, &mut out);
    Ok(out)
}
