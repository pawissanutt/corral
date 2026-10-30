//! Plan-only review progression. No code gate or integration path is reachable.
use anyhow::{Context, Result};
use corral_core::{PlanRunState, Status, Task, TaskHead, TaskRef as _, Verdict};
use jiff::Timestamp;
use serde_json::json;

use crate::{Ctx, plan_dispatch};

#[cfg(test)]
#[path = "plan_pilot_tests.rs"]
mod tests;

pub(crate) fn reconcile(
    ctx: &Ctx,
    task: &Task,
    snapshot: crate::pane_liveness::Snapshot<'_>,
    now: Timestamp,
) -> Result<()> {
    let panes = snapshot.panes;
    let _attribution = ctx.store.attribute(corral_core::Actor::System);
    let Some(run) = task.plan_runs.last().filter(|r| r.holds_slot()) else {
        return Ok(());
    };
    let pane_id = run.herdr.as_ref().map(|h| h.pane_id.as_str());
    let pane = pane_id.and_then(|id| panes.get(id));
    if let Some(reason) = crate::plan_lifecycle::exit_reason(run)? {
        crate::plan_lifecycle::fail(ctx, task, &reason, now)?;
        return Ok(());
    }
    let gone = crate::pane_liveness::gone(
        ctx,
        crate::pane_liveness::Sample {
            task: task.into(),
            pane: pane_id,
            started: run.started,
            seen: false,
            present: pane.is_some(),
            at: now,
            ticket: snapshot.ticket,
            count_miss: true,
        },
    )?;
    if gone {
        crate::plan_lifecycle::fail(
            ctx,
            task,
            "Planner pane disappeared before plan submission.",
            now,
        )?;
    } else if let Some(pane) = pane {
        let grace = i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap_or(i64::MAX);
        if now.duration_since(run.started).as_secs() >= grace {
            match crate::plan_lifecycle::shell_returned(ctx, run) {
                Ok(true) => {
                    crate::plan_lifecycle::fail(
                        ctx,
                        task,
                        "Planner returned to a shell before plan submission.",
                        now,
                    )?;
                    return Ok(());
                }
                Ok(false) => {}
                Err(error) => crate::messages::report(
                    Some(&task.id),
                    "planner process observation",
                    format!("{error:#}"),
                ),
            }
        }
        let status = match pane.status {
            corral_herdr::AgentStatus::Blocked => Status::NeedsMe,
            corral_herdr::AgentStatus::Working => Status::Running,
            corral_herdr::AgentStatus::Idle
            | corral_herdr::AgentStatus::Done
            | corral_herdr::AgentStatus::Unknown => task.status,
        };
        // A planner question that waits on the owner holds Needs me like a worker's: the pane's
        // working state never clears that hold; answering or withdrawing the question does.
        let planner = corral_core::agent::ContainerRef::Planner {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let question_hold = corral_core::agent::container_state(task, &planner)
            .is_some_and(|state| state.question_hold.is_some());
        if task.status != status && !question_hold {
            ctx.store.update_head(&task.id, |t| {
                t.status = status;
                ctx.store.log_event(
                    Some(&task.id),
                    "plan_agent_state",
                    json!({"target":"plan","run":run.id,"status":status}),
                )?;
                Ok(())
            })?;
        }
    }
    Ok(())
}

/// [`handles`] of any task form: its current run and last planning run.
pub(crate) fn handles_ref<T: corral_core::TaskRef + ?Sized>(task: &T) -> Result<bool> {
    if task
        .current_run_head()?
        .is_some_and(|r| r.outcome.is_none())
    {
        return Ok(false);
    }
    Ok(task
        .last_plan_run_head()?
        .is_some_and(|r| r.state != PlanRunState::Approved))
}

pub(crate) fn handles(task: &Task) -> bool {
    if task.current_run().is_some_and(|r| r.outcome.is_none()) {
        return false;
    }
    task.plan_runs
        .last()
        .is_some_and(|r| r.state != PlanRunState::Approved)
        || (task.status == Status::Review && task.current_run().is_none() && !task.plans.is_empty())
}

pub(crate) fn advance_all(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Autopilot);
    for task in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        let Some(_lock) = ctx.store.try_lock(&format!("plan-pilot-{}", task.id))? else {
            continue;
        };
        if let Err(error) = advance(ctx, &task.id, now) {
            crate::messages::report(Some(&task.id), "plan autopilot", format!("{error:#}"));
        }
    }
    Ok(())
}

/// Whether the planner still owns the current planning attempt.
fn planner_owns(task: &Task) -> bool {
    task.plan_runs.last().is_some_and(|r| {
        matches!(
            r.state,
            PlanRunState::Planning
                | PlanRunState::Failed
                | PlanRunState::Escalated
                | PlanRunState::Cancelled
        ) || (r.state == PlanRunState::Changes && task.status == Status::Running)
    })
}

fn advance(ctx: &Ctx, id: &str, now: Timestamp) -> Result<()> {
    let shell = ctx.store.head(id)?.to_shell_task()?;
    if shell.park.is_some() || idle(ctx, &shell, now)? {
        return Ok(());
    }
    act(ctx, id, now)
}

/// `act`'s decision from the head alone, which holds the last plan with its current review and
/// the last plan run: `true` only where `act` returns without writing, the same error where
/// it refuses before any write, and `false` (the full path) wherever it may act.
fn idle(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<bool> {
    if plan_dispatch::may_resume(task)? {
        return Ok(false);
    }
    if planner_owns(task) {
        return Ok(true);
    }
    let Some(plan) = task.plans.last() else {
        return Ok(true);
    };
    crate::plan::config(ctx)?;
    if crate::plan_review::has_prepared(ctx, &task.id, &plan.id)? {
        return Ok(false);
    }
    if plan.approval.is_some() {
        return Ok(task.plan.as_deref() == Some(&format!("{}@{}", task.id, plan.id)));
    }
    crate::plan::fresh(ctx, task, plan)?;
    let Some(review) = plan.reviews.last() else {
        crate::operating::require(
            &ctx.store,
            task,
            corral_core::OperatingActionKind::PlanReview,
        )?;
        return Ok(false);
    };
    Ok(match review.verdict {
        None => match &review.herdr {
            None => true,
            Some(herdr) => {
                let grace =
                    i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap_or(i64::MAX);
                now.duration_since(review.at).as_secs() <= grace
                    || ctx.herdr.pane_get(&herdr.pane_id)?.is_some()
            }
        },
        Some(Verdict::Approve) => {
            crate::operating::require(
                &ctx.store,
                task,
                corral_core::OperatingActionKind::AcceptPlan,
            )
            .is_err()
                && task
                    .plan_runs
                    .last()
                    .is_some_and(|r| r.state == PlanRunState::AwaitingOwner)
        }
        Some(Verdict::Changes) if crate::queue::queued_plan_requested(task) => true,
        Some(Verdict::Changes) => {
            crate::operating::require(&ctx.store, task, corral_core::OperatingActionKind::Plan)?;
            false
        }
        Some(Verdict::Escalate) => false,
    })
}

fn act(ctx: &Ctx, id: &str, now: Timestamp) -> Result<()> {
    let head = ctx.store.head(id)?;
    // The decision reads head fields, the last plan with its current review and the last
    // planner run, which the head's shell holds.
    let task = &head.shell_task()?;
    let config = crate::plan::config(ctx)?;
    // Prepared actions retain their exact round and owner/automatic provenance. A
    // failed authorization is not a planner exit, nor a reason to claim a round.
    if plan_dispatch::resume_prepared(ctx, &head)? {
        return Ok(());
    }
    let run = task.plan_runs.last();
    if planner_owns(task) {
        return Ok(());
    }
    let Some(plan) = task.plans.last() else {
        return Ok(());
    };
    if crate::plan_review::resume_prepared(ctx, task)? {
        return Ok(());
    }
    if plan.approval.is_some() {
        if task.plan.as_deref() != Some(&format!("{}@{}", task.id, plan.id)) {
            crate::plan::fresh(ctx, task, plan)?;
            if plan
                .approval
                .as_ref()
                .is_some_and(|approval| approval.by == "owner")
            {
                crate::plan_decompose::apply(ctx, &task.id, &plan.id)?;
            } else {
                ctx.store
                    .reconcile_plan_acceptance_head(&task.id, |store, current| {
                        crate::operating::require(
                            store,
                            current,
                            corral_core::OperatingActionKind::AcceptPlan,
                        )?;
                        crate::plan_decompose::check_attachment(store, current, &plan.id)?;
                        crate::plan_decompose::check_scopes(store, current, &plan.id)
                    })?;
                crate::plan_decompose::apply_auto(ctx, &task.id, &plan.id)?;
            }
        }
        return Ok(());
    }
    crate::plan::fresh(ctx, task, plan)?;
    let Some(review) = plan.reviews.last() else {
        crate::operating::require(
            &ctx.store,
            task,
            corral_core::OperatingActionKind::PlanReview,
        )?;
        if let Err(error) = crate::plan::expected_reviewer(&config, task) {
            return escalate(ctx, &head, &format!("{error:#}"));
        }
        crate::plan_review::review_auto(ctx, &task.id)?;
        return Ok(());
    };
    match review.verdict {
        None => {
            if let Some(herdr) = &review.herdr {
                let grace =
                    i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap_or(i64::MAX);
                if now.duration_since(review.at).as_secs() > grace
                    && ctx.herdr.pane_get(&herdr.pane_id)?.is_none()
                {
                    escalate(
                        ctx,
                        &head,
                        "plan reviewer disappeared before recording a verdict",
                    )?;
                }
            }
        }
        Some(Verdict::Approve) => {
            if crate::operating::require(
                &ctx.store,
                task,
                corral_core::OperatingActionKind::AcceptPlan,
            )
            .is_ok()
            {
                crate::plan_review::accept_auto(ctx, &task.id, &plan.id)?;
            } else if run.is_none_or(|r| r.state != PlanRunState::AwaitingOwner)
                && (ctx.store.effective_operating_policy()?.mode
                    == corral_core::OperatingMode::Assist
                    || !config.automation.authority.plan_accept
                    || config.explicit_owner_acceptance(task, true)
                    || crate::plan::owner_decisions(task, plan))
            {
                ctx.store.update_head(&task.id, |t| {
                    if !same_plan_state(t.head(), &head) {
                        return Ok(());
                    }
                    if let Some(run) = t.last_plan_run_mut() {
                        run.state = PlanRunState::AwaitingOwner;
                    }
                    t.status = Status::NeedsMe;
                    t.queued = false;
                    ctx.store.log_event(
                        Some(&task.id),
                        "plan_awaiting_owner",
                        json!({"target":"plan","plan":plan.id}),
                    )?;
                    Ok(())
                })?;
            }
        }
        // An explicitly queued owner planner request drains through the queue instead.
        Some(Verdict::Changes) if crate::queue::queued_plan_requested(task) => {}
        Some(Verdict::Changes) => {
            crate::operating::require(&ctx.store, task, corral_core::OperatingActionKind::Plan)?;
            let Some(run) = run else {
                return escalate(
                    ctx,
                    &head,
                    "plan changes require a planner; dispatch a plan run",
                );
            };
            let max = ctx
                .store
                .config
                .review_policy(task.project.as_deref(), task.review)
                .max_rounds;
            if run.rounds >= max {
                return escalate(ctx, &head, "plan review reached max_rounds");
            }
            plan_dispatch::revise(
                ctx,
                &head,
                review
                    .notes
                    .as_deref()
                    .context("plan change verdict has no notes")?,
                crate::operating::Origin::Automatic,
            )?;
        }
        Some(Verdict::Escalate) => escalate(
            ctx,
            &head,
            review.notes.as_deref().unwrap_or("plan review escalated"),
        )?,
    }
    Ok(())
}

/// The plan state a decision was taken on is still current: the last plan with its current
/// review and facts, the last planner run and the planner run count, as the full comparison of
/// the last plan and every planner run decided.
fn same_plan_state(current: &TaskHead, seen: &TaskHead) -> bool {
    current.last_plan == seen.last_plan
        && current.last_plan_run == seen.last_plan_run
        && current.summary.plan_runs == seen.summary.plan_runs
}

fn escalate(ctx: &Ctx, head: &TaskHead, reason: &str) -> Result<()> {
    let mut escalated = false;
    ctx.store.update_head(&head.id, |t| {
        // A concurrent owner verdict or review retry supersedes this observation.
        if !same_plan_state(t.head(), head)
            || t.head()
                .last_plan_run
                .as_ref()
                .is_some_and(|r| r.state == PlanRunState::Cancelled)
        {
            return Ok(());
        }
        if let Some(plan) = t.head().last_plan.as_ref().map(|p| p.id.clone()) {
            t.terminalize_pending_plan_reviews(&plan, reason)?;
        }
        t.status = Status::NeedsMe;
        t.queued = false;
        if let Some(run) = t.last_plan_run_mut() {
            run.state = PlanRunState::Escalated;
            run.pending_reply = Some(reason.into());
        }
        escalated = true;
        ctx.store.log_event(
            Some(&head.id),
            "plan_escalated",
            json!({"target":"plan","reason":reason}),
        )?;
        Ok(())
    })?;
    if !escalated {
        return Ok(());
    }
    Ok(())
}
