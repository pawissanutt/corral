//! One ordering for explicit requests, native queued gates and automatic finishing work.
use anyhow::Result;
use corral_core::{AutoStage, GateResult, ReviewWork, RunOutcome, Status, Store, TaskHead};

use crate::Ctx;

pub(crate) fn key(task: &TaskHead) -> (&str, jiff::Timestamp, &str) {
    (&task.priority, task.created, &task.id)
}

fn waiting(ctx: &Ctx, task: &TaskHead) -> Result<Option<ReviewWork>> {
    if task.hold.is_some() || task.park.is_some() {
        return Ok(None);
    }
    if let Some(plan) = task.last_plan.as_deref()
        && plan.approval.is_none()
        && plan.current_review.as_ref().is_none_or(|r| r.cancelled)
        && crate::plan_pilot::handles_ref(task)?
        && crate::operating::require(
            &ctx.store,
            task,
            corral_core::OperatingActionKind::PlanReview,
        )
        .is_ok()
    {
        return Ok(Some(ReviewWork::Reviewer));
    }
    if let Some(attempt) = task.last_aggregate_attempt.as_deref()
        && (attempt.gate.as_ref().is_some_and(|g| g.holds_slot())
            || attempt.current_review.as_ref().is_some_and(|r| r.pending()))
    {
        return Ok(None);
    }
    if let Some(work) = task.review_wait {
        if let Some(run) = task.current_run.as_deref()
            && super::retained_preparation(&ctx.store, &task.id, &run.id, work)?
        {
            return Ok(None);
        }
        return Ok(Some(work));
    }
    let Some(run) = task.current_run.as_deref() else {
        return Ok(None);
    };
    if run.gate.as_ref().is_some_and(|g| g.holds_slot())
        || run.review.as_ref().is_some_and(|r| r.holds_slot())
    {
        return Ok(None);
    }
    if task.status != Status::Review || run.outcome != Some(RunOutcome::Done) {
        return Ok(None);
    }
    if matches!(
        run.autopilot.as_ref().map(|s| s.stage),
        Some(AutoStage::Escalated | AutoStage::Done | AutoStage::Changes)
    ) {
        return Ok(None);
    }
    let policy = ctx.store.config.task_review_policy(task);
    let work = if !policy.gate.is_empty()
        && run
            .gate
            .as_ref()
            .is_none_or(|g| g.result != GateResult::Pass)
    {
        Some(ReviewWork::Gate)
    } else if policy.reviewer != corral_core::Reviewer::None && run.review.is_none() {
        Some(ReviewWork::Reviewer)
    } else {
        None
    };
    if let Some(work) = work
        && super::retained_preparation(&ctx.store, &task.id, &run.id, work)?
    {
        return Ok(None);
    }
    let action = match work {
        Some(ReviewWork::Gate) => corral_core::OperatingActionKind::Gate,
        Some(ReviewWork::Reviewer) => corral_core::OperatingActionKind::Review,
        None => return Ok(None),
    };
    Ok(crate::operating::require(&ctx.store, task, action)
        .is_ok()
        .then_some(work)
        .flatten())
}

/// Checked under the atomic admission claim, even if a lower-priority caller got the
/// launch lease first. No lower-priority request may steal a waiting task's turn.
pub(crate) fn ahead(store: &Store, task: &TaskHead) -> Result<Option<String>> {
    let ctx = Ctx::from_store(store.clone());
    let mut tasks = crate::pm_next::heads::open(store)?.as_ref().clone();
    tasks.sort_by(|a, b| key(a).cmp(&key(b)));
    for other in tasks {
        if key(&other) >= key(task) {
            break;
        }
        if other.id != task.id && waiting(&ctx, &other)?.is_some() {
            return Ok(Some(format!(
                "{} ({}) has priority for the next review slot",
                other.id, other.priority
            )));
        }
    }
    Ok(None)
}

/// Admission for auxiliary Plan gates/reviews, using the same lease and ordering as runs.
/// Keep the lease through publishing the Running/pending marker, not through gate execution.
pub(crate) fn auxiliary(ctx: &Ctx, id: &str, kind: ReviewWork) -> Result<corral_core::StoreLock> {
    let lease = ctx
        .store
        .try_lock("review-budget")?
        .ok_or(super::LaunchBusy("another review launch is in progress"))?;
    let task = ctx.store.head(id)?;
    crate::hold::require_clear(&ctx.store, &task)?;
    let credit = usize::from(
        kind == ReviewWork::Reviewer
            && task
                .last_aggregate_attempt
                .as_ref()
                .and_then(|a| a.current_review.as_ref())
                .is_some_and(|r| r.pending()),
    );
    let mut active = super::active_processes(ctx, id, kind)?.saturating_sub(credit);
    if preempt(ctx, &task, active, false)? {
        active = super::active_processes(ctx, id, kind)?.saturating_sub(credit);
    }
    let reason = ahead(&ctx.store, &task)?
        .or(crate::resources::admission(ctx)?)
        .or_else(|| {
            (active >= ctx.store.config.limits.review).then(|| {
                format!(
                    "review process limit reached ({active}/{})",
                    ctx.store.config.limits.review
                )
            })
        });
    if let Some(reason) = reason {
        let wait = super::CapacityWait { kind, reason };
        wait.persist(ctx, id, ctx.store.now())?;
        return Err(wait.into());
    }
    Ok(lease)
}

/// The caller owns review-budget; only gates (never worker panes) can be preempted.
pub(crate) fn preempt(ctx: &Ctx, task: &TaskHead, active: usize, force: bool) -> Result<bool> {
    let Some(threshold) = ctx.store.config.limits.preempt.as_deref() else {
        return Ok(false);
    };
    if task.priority.as_str() > threshold || ahead(&ctx.store, task)?.is_some() {
        return Ok(false);
    }
    if active < ctx.store.config.limits.review
        && (force || crate::resources::admission(ctx)?.is_none())
    {
        return Ok(false);
    }
    let tasks = crate::pm_next::heads::open(&ctx.store)?;
    let victim = tasks
        .iter()
        .filter(|other| {
            other.priority > task.priority
                && (other
                    .current_run
                    .as_ref()
                    .and_then(|r| r.gate.as_ref())
                    .is_some_and(|g| g.holds_slot())
                    || other
                        .last_aggregate_attempt
                        .as_ref()
                        .and_then(|a| a.gate.as_ref())
                        .is_some_and(|g| g.holds_slot())
                    || other.review_reservation == Some(ReviewWork::Gate))
        })
        .max_by(|a, b| key(a).cmp(&key(b)));
    let Some(victim) = victim else {
        return Ok(false);
    };
    super::cancel_gates(
        ctx,
        &victim.id,
        &format!("preempted by {} ({})", task.id, task.priority),
        true,
    )?;
    Ok(true)
}
