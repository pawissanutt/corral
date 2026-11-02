//! Full-task counterpart of indexed unfinished native-container discovery.
use super::{AgentRunState, ContainerRef};
use crate::{NativeRecordCounts, Task};

pub(crate) fn active_worker_wakes(task: &Task) -> Vec<(ContainerRef, super::WakeSubscription)> {
    task.runs
        .iter()
        .flat_map(|run| {
            run.agent_host.iter().flat_map(move |state| {
                state
                    .wakes
                    .iter()
                    .filter(|wake| {
                        matches!(
                            wake.state,
                            super::WakeState::Armed | super::WakeState::PendingStart
                        )
                    })
                    .cloned()
                    .map(move |wake| {
                        (
                            ContainerRef::Worker {
                                task: task.id.clone(),
                                run: run.id.clone(),
                            },
                            wake,
                        )
                    })
            })
        })
        .collect()
}

pub(crate) fn worker_queued_effect_receipt(
    task: &Task,
    reference: &str,
) -> Option<super::RequestReceipt> {
    task.runs
        .iter()
        .filter_map(|run| run.agent_host.as_deref())
        .flat_map(|state| &state.receipts)
        .find(|receipt| {
            receipt
                .effect
                .as_ref()
                .is_some_and(|effect| effect.reference == reference)
                && receipt.result["queued"] == true
        })
        .cloned()
}

fn owes(native: bool, settled: Option<jiff::Timestamp>, host: Option<&AgentRunState>) -> bool {
    native
        && (settled.is_none()
            || host.is_some_and(|state| NativeRecordCounts::of(state).pending > 0))
}

pub(crate) fn unfinished(task: &Task) -> Vec<ContainerRef> {
    let mut out = Vec::new();
    for run in &task.runs {
        if owes(
            run.agent_host.is_some() || run.harness.as_deref().is_some_and(crate::is_corral_pi),
            run.inbox.settled,
            run.agent_host.as_deref(),
        ) {
            out.push(ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            });
        }
        for child in &run.review_runs {
            if owes(
                child.agent_host.is_some()
                    || child.harness.as_deref().is_some_and(crate::is_corral_pi),
                child.inbox.settled,
                child.agent_host.as_deref(),
            ) {
                out.push(ContainerRef::Reviewer {
                    task: task.id.clone(),
                    run: run.id.clone(),
                    review: child.id.clone(),
                });
            }
        }
    }
    for run in &task.plan_runs {
        if owes(
            run.agent_host.is_some() || crate::is_corral_pi(&run.harness),
            run.inbox.settled,
            run.agent_host.as_deref(),
        ) {
            out.push(ContainerRef::Planner {
                task: task.id.clone(),
                run: run.id.clone(),
            });
        }
    }
    for plan in &task.plans {
        for review in &plan.reviews {
            if owes(
                review.agent_host.is_some(),
                review.inbox.settled,
                review.agent_host.as_deref(),
            ) {
                out.push(ContainerRef::PlanReviewer {
                    task: task.id.clone(),
                    plan: plan.id.clone(),
                    review: review.id.clone(),
                });
            }
        }
    }
    out
}
