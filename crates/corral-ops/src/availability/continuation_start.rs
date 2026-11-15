//! A failed first bind consumes the prepared launch, not another recovery attempt.
use super::{IntentPhase, PauseReason, intent_for, pause};
use crate::Ctx;
use anyhow::Result;
use corral_core::{
    Status, TaskRef as _,
    agent::{ClosureReason, ContainerRef, container_closure_ref},
};

/// Call only after detecting failure on a potentially paid native attempt. The keyed
/// intent lookup excludes ordinary starts. A Paused intent still owes the separate task
/// hold after a crash between commits; repeating that hold must not publish another write.
/// Until the successor record exists, only its exact still-current source may be held.
/// Returning true also keeps subsequent watchdog passes out of legacy terminalization,
/// which would replace the durable Paused reason with a synthetic result or Closed.
pub(crate) fn failed_start(ctx: &Ctx, container: &ContainerRef, generation: u32) -> Result<bool> {
    let Some(intent) = intent_for(&ctx.store, container, generation)? else {
        return Ok(false);
    };
    if !matches!(intent.phase, IntentPhase::Prepared | IntentPhase::Paused) {
        return Ok(false);
    }
    if intent.phase == IntentPhase::Prepared {
        // Pause owns the successor even when its native state has not been created.
        pause(
            ctx,
            container,
            PauseReason::SuccessorAmbiguous,
            "successor host lost before healthy bind",
        )?;
    }
    if let Some(task) = container.task() {
        ctx.store.try_update_head(task, |head| {
            if matches!(
                head.status,
                Status::NeedsMe | Status::Done | Status::Dropped
            ) {
                return Ok(false);
            }
            let successor_exists = match container {
                ContainerRef::Worker { run, .. } => head.run_head(run)?.is_some(),
                ContainerRef::Reviewer { run, review, .. } => {
                    head.review_child(run, review)?.is_some()
                }
                ContainerRef::Planner { run, .. } => head.plan_run_head(run)?.is_some(),
                ContainerRef::PlanReviewer { plan, review, .. } => {
                    head.plan_review(plan, review)?.is_some()
                }
                ContainerRef::Pm | ContainerRef::Assistant { .. } => false,
            };
            let owner = if successor_exists {
                &intent.successor
            } else {
                &intent.source
            };
            let closure = container_closure_ref(&*head, &owner.container)?;
            // Preparation may interrupt/cancel its current review source before allocation.
            // That is not an authoritative verdict or permission to hold a replaced child.
            let interrupted_source = !successor_exists
                && match (&owner.container, closure) {
                    (
                        ContainerRef::Reviewer { run, review, .. },
                        Some(ClosureReason::ReviewInterrupted),
                    ) => head.current_run_head()?.is_some_and(|parent| {
                        parent.id == *run
                            && parent
                                .current_child
                                .as_ref()
                                .is_some_and(|c| c.id == *review)
                    }),
                    (
                        ContainerRef::PlanReviewer { plan, review, .. },
                        Some(ClosureReason::ReviewClosed),
                    ) => head.last_plan_head()?.is_some_and(|parent| {
                        parent.id == *plan
                            && parent.current_review.as_ref().is_some_and(|child| {
                                child.id == *review && child.cancelled && child.verdict.is_none()
                            })
                    }),
                    _ => false,
                };
            let host = head.agent_head(&owner.container)?;
            if owner.container.task() != Some(task)
                || (closure.is_some() && !interrupted_source)
                || (!successor_exists && host.is_none())
                || host.is_some_and(|host| {
                    host.binding.generation != owner.generation || host.binding.closing
                })
            {
                return Ok(false);
            }
            head.status = Status::NeedsMe;
            Ok(true)
        })?;
    }
    Ok(true)
}
