//! A failed first bind consumes the prepared launch, not another recovery attempt.
use super::{IntentPhase, PauseReason, intent_for, pause};
use crate::Ctx;
use anyhow::Result;
use corral_core::{
    Status, TaskRef as _,
    agent::{ClosureReason, ContainerRef, container_closure_ref},
};

pub(super) fn session_may_start(ctx: &Ctx, container: &ContainerRef) -> Result<bool> {
    let recipient = match container {
        ContainerRef::Pm => corral_core::inbox::Recipient::Pm,
        ContainerRef::Assistant { session } => corral_core::inbox::Recipient::Assistant {
            session: session.clone(),
        },
        container => crate::questions::recipient(container)
            .ok_or_else(|| anyhow::anyhow!("continuation recipient missing"))?,
    };
    if !crate::session_control::automation_may_start(ctx, &recipient)? {
        return Ok(false);
    }
    if let ContainerRef::Worker { task, run } = container
        && ctx
            .store
            .head(task)?
            .run_head(run)?
            .is_some_and(|run| run.outcome == Some(corral_core::RunOutcome::Stopped))
    {
        return Ok(false);
    }
    Ok(true)
}

/// Ordinary owner starts have no incoming financial intent. A prepared replacement,
/// however, must not use its new container identity to escape its source session fence.
pub(crate) fn automation_may_start(ctx: &Ctx, container: &ContainerRef) -> Result<bool> {
    let Some(intent) = super::latest_for(&ctx.store, container)?.filter(|intent| {
        intent.phase == IntentPhase::Prepared && intent.successor.container == *container
    }) else {
        return Ok(true);
    };
    Ok(session_may_start(ctx, container)? && session_may_start(ctx, &intent.source.container)?)
}

pub(crate) fn require_automation_start(ctx: &Ctx, container: &ContainerRef) -> Result<()> {
    if !automation_may_start(ctx, container)? {
        return Err(crate::operating::AuthorizationRefused(
            "Held: session controls defer paid host continuation".into(),
        )
        .into());
    }
    Ok(())
}

/// Resume may follow hello: only its exact bound Prepared generation still owes activation.
pub(crate) fn activate_ready(ctx: &Ctx, container: &ContainerRef) -> Result<()> {
    let Some(intent) = super::latest_for(&ctx.store, container)?.filter(|intent| {
        intent.phase == IntentPhase::Prepared && intent.successor.container == *container
    }) else {
        return Ok(());
    };
    let generation = intent.successor.generation;
    let ready = if let Some(task) = container.task() {
        let head = ctx.store.head(task)?;
        head.agent_head(container)?.is_some_and(|host| {
            host.binding.generation == generation
                && !host.binding.closing
                && host.binding.health == corral_core::agent::HostHealth::Healthy
        }) && head
            .agent_generation(container, generation)?
            .is_some_and(|generation| generation.bound.is_some() && generation.fenced.is_none())
    } else {
        let bound = |state: &corral_core::agent::AgentRunState| {
            state.binding.generation == generation
                && !state.binding.closing
                && state.binding.health == corral_core::agent::HostHealth::Healthy
                && state.current_generation().is_some_and(|generation| {
                    generation.bound.is_some() && generation.fenced.is_none()
                })
        };
        match container {
            ContainerRef::Pm => corral_core::agent::pm_state::load_current(&ctx.store)?
                .is_some_and(|pm| bound(&pm.state)),
            ContainerRef::Assistant { session } => {
                bound(&ctx.store.get_assistant_container(session)?.state)
            }
            _ => false,
        }
    };
    if !ready {
        return Ok(());
    }
    if let Err(error) = super::activate(ctx, container, generation) {
        if error.is::<crate::operating::AuthorizationRefused>()
            || !automation_may_start(ctx, container)?
        {
            return Ok(());
        }
        pause(
            ctx,
            container,
            PauseReason::SuccessorAmbiguous,
            "activation failed after resume",
        )?;
    }
    Ok(())
}

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
