//! Full-task counterpart of indexed unfinished native-container discovery.
use super::{AgentRunState, ContainerRef};
use crate::{NativeRecordCounts, Task};

fn owes(native: bool, settled: Option<jiff::Timestamp>, host: Option<&AgentRunState>) -> bool {
    native
        && (settled.is_none()
            || host.is_some_and(|state| NativeRecordCounts::of(state).pending > 0))
}

pub(crate) fn unfinished(task: &Task) -> Vec<ContainerRef> {
    let mut out = Vec::new();
    for run in &task.runs {
        if owes(
            run.agent_host.is_some() || run.harness.as_deref().is_some_and(crate::is_pi_native),
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
                    || child.harness.as_deref().is_some_and(crate::is_pi_native),
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
            run.agent_host.is_some() || crate::is_pi_native(&run.harness),
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
