//! Record settlement is separate from host/pane effects so retries can finish
//! any interrupted boundary without treating an owner's stop as host failure.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{ContainerRef, pm_state};
use corral_core::{PlanRunState, RunOutcome, Status, TaskRef as _};

use super::super::Target;
use crate::Ctx;

pub(super) fn run_id(target: &Target) -> Option<&str> {
    match target.container.as_ref() {
        Some(
            ContainerRef::Worker { run, .. }
            | ContainerRef::Reviewer { run, .. }
            | ContainerRef::Planner { run, .. },
        ) => Some(run),
        _ => None,
    }
}

pub(super) fn check_generation(ctx: &Ctx, target: &Target) -> Result<()> {
    let Some(expected) = target.generation else {
        return Ok(());
    };
    let container = target
        .container
        .as_ref()
        .context("no_bridge: missing container")?;
    let actual = if let Some(task) = target.task.as_deref() {
        ctx.store
            .head(task)?
            .agent_head(container)?
            .map(|host| host.binding.generation)
    } else {
        pm_state::load_current(&ctx.store)?.map(|pm| pm.state.binding.generation)
    };
    ensure!(
        actual == Some(expected),
        "conflict: stopped session generation changed to {actual:?}"
    );
    Ok(())
}

pub(super) fn stop_children(ctx: &Ctx, target: &Target) -> Result<()> {
    let id = target.task.as_deref().context("worker task missing")?;
    let head = ctx.store.head(id)?;
    let run = head
        .current_run
        .as_deref()
        .filter(|run| Some(run.id.as_str()) == run_id(target))
        .context("conflict: worker run changed before stop")?;
    // All open children, not only the latest child, can retain native job groups.
    for child in head.open_review_children(&run.id)? {
        let child_target =
            super::super::resolve(ctx, &format!("{id}/review/{}/{}", run.id, child.id))?;
        if child_target.bridge {
            let container = child_target
                .container
                .as_ref()
                .context("review host identity missing")?;
            let waiting = crate::agent::recovery::stop_container(ctx, container, true)?;
            ensure!(
                waiting.is_empty(),
                "still_alive: review {}: {}",
                child.id,
                waiting.join(", ")
            );
        }
        crate::review_budget::cancel_reviewer(ctx, id, &run.id, &child.id, "stopped by owner")?;
        super::close_pane(ctx, &child_target)?;
    }
    // Existing cancellation also retires retained preparations and active gates.
    crate::review_budget::cancel_task(ctx, id, "stopped by owner")
        .context("cancelling worker review children and gates")
}

pub(super) fn records(ctx: &Ctx, target: &Target) -> Result<()> {
    let Some(id) = target.task.as_deref() else {
        return Ok(());
    };
    match target.role.as_str() {
        "worker" => {
            let run = run_id(target).context("worker run missing")?;
            ctx.store.update(id, |task| {
                let current = task
                    .runs
                    .iter_mut()
                    .find(|current| current.id == run)
                    .context("worker run disappeared")?;
                ensure!(
                    current.outcome.is_none() || current.outcome == Some(RunOutcome::Stopped),
                    "ended: worker already has another outcome"
                );
                current.outcome = Some(RunOutcome::Stopped);
                current.ended.get_or_insert(ctx.store.now());
                current.pending_reply = None;
                current.usage_pending = true;
                task.status = Status::NeedsMe;
                task.queued = false;
                task.review_wait = None;
                task.review_reservation = None;
                Ok(())
            })?;
        }
        "reviewer" => {
            let Some(ContainerRef::Reviewer { run, review, .. }) = target.container.as_ref() else {
                anyhow::bail!("invalid_session: reviewer identity missing");
            };
            crate::review_budget::cancel_reviewer(ctx, id, run, review, "stopped by owner")?;
        }
        "gate" => {
            crate::gate::cancel_without_pane(ctx, id, "stopped by owner")?;
        }
        "planner" => {
            let run = run_id(target).context("planner run missing")?;
            ctx.store.update(id, |task| {
                let current = task
                    .plan_runs
                    .iter_mut()
                    .find(|current| current.id == run)
                    .context("planner run disappeared")?;
                // Cancelled already means intentional owner termination and blocks
                // automatic re-planning; its retained note records the precise reason.
                current.state = PlanRunState::Cancelled;
                current.ended.get_or_insert(ctx.store.now());
                current.pending_reply = Some("Stopped by owner.".into());
                task.status = Status::NeedsMe;
                task.queued = false;
                task.dispatch_plan = None;
                task.dispatch_hold = true;
                Ok(())
            })?;
        }
        "plan_reviewer" => {
            let Some(ContainerRef::PlanReviewer { plan, review, .. }) = target.container.as_ref()
            else {
                anyhow::bail!("invalid_session: plan reviewer identity missing");
            };
            crate::plan_review::cancel_attempt(ctx, id, plan, review, "stopped by owner")?;
        }
        role => anyhow::bail!("invalid_session: unsupported role {role}"),
    }
    Ok(())
}

pub(super) fn remaining_processes(ctx: &Ctx, target: &Target) -> Result<()> {
    if target.role == "worker" {
        crate::cleanup_worktree::stop_retained_run(
            ctx,
            target.task.as_deref().context("worker task missing")?,
            run_id(target).context("worker run missing")?,
            target.bridge,
        )?;
    }
    Ok(())
}
