//! Plan-only review progression. No code gate or integration path is reachable.
use anyhow::{Context, Result};
use corral_core::{PlanRunState, Status, Task, Verdict};
use jiff::Timestamp;
use serde_json::json;

use crate::{Ctx, plan_dispatch};

#[cfg(test)]
#[path = "plan_pilot_tests.rs"]
mod tests;

pub(crate) fn reconcile(
    ctx: &Ctx,
    task: &Task,
    panes: &std::collections::HashMap<String, corral_herdr::PaneInfo>,
    now: Timestamp,
) -> Result<()> {
    let Some(run) = task.plan_runs.last().filter(|r| r.holds_slot()) else {
        return Ok(());
    };
    let pane_id = run.herdr.as_ref().map(|h| h.pane_id.as_str());
    let pane = pane_id.and_then(|id| panes.get(id));
    let gone = crate::pane_liveness::gone(
        ctx,
        crate::pane_liveness::Sample {
            task,
            pane: pane_id,
            started: run.started,
            seen: false,
            present: pane.is_some(),
            at: now,
            count_miss: true,
        },
    )?;
    if gone {
        ctx.store.update(&task.id, |t| {
            let current = t
                .plan_runs
                .last_mut()
                .filter(|r| r.id == run.id && r.holds_slot())
                .context("planner changed during reconciliation")?;
            current.state = PlanRunState::Failed;
            current.ended = Some(now);
            current.pending_reply = Some("Planner pane disappeared before plan submission.".into());
            t.status = Status::NeedsMe;
            Ok(())
        })?;
        ctx.store.log_event(
            Some(&task.id),
            "plan_failed",
            json!({"target":"plan","run":run.id,"reason":"planner pane disappeared"}),
        )?;
    } else if let Some(pane) = pane {
        let status = match pane.status {
            corral_herdr::AgentStatus::Blocked => Status::NeedsMe,
            corral_herdr::AgentStatus::Working => Status::Running,
            corral_herdr::AgentStatus::Idle
            | corral_herdr::AgentStatus::Done
            | corral_herdr::AgentStatus::Unknown => task.status,
        };
        if task.status != status {
            ctx.store.update(&task.id, |t| {
                t.status = status;
                Ok(())
            })?;
            ctx.store.log_event(
                Some(&task.id),
                "plan_agent_state",
                json!({"target":"plan","run":run.id,"status":status}),
            )?;
        }
    }
    Ok(())
}

pub(crate) fn handles(task: &Task) -> bool {
    task.plan_runs
        .last()
        .is_some_and(|r| r.state != PlanRunState::Approved)
        || (task.status == Status::Review && task.current_run().is_none() && !task.plans.is_empty())
}

pub(crate) fn advance_all(ctx: &Ctx, now: Timestamp) -> Result<()> {
    if !ctx.store.config.autopilot_enabled() {
        return Ok(());
    }
    for task in ctx.store.list()? {
        if matches!(task.status, Status::Done | Status::Dropped) {
            continue;
        }
        let Some(_lock) = ctx.store.try_lock(&format!("plan-pilot-{}", task.id))? else {
            continue;
        };
        if let Err(error) = advance(ctx, &task, now) {
            crate::messages::report(Some(&task.id), "plan autopilot", format!("{error:#}"));
        }
    }
    Ok(())
}

fn advance(ctx: &Ctx, task: &Task, now: Timestamp) -> Result<()> {
    let run = task.plan_runs.last();
    if run.is_some_and(|r| {
        matches!(
            r.state,
            PlanRunState::Planning | PlanRunState::Failed | PlanRunState::Escalated
        ) || (r.state == PlanRunState::Changes && task.status == Status::Running)
    }) {
        return Ok(());
    }
    let Some(plan) = task.plans.last() else {
        return Ok(());
    };
    if plan.approval.is_some() {
        if task.plan.as_deref() != Some(&format!("{}@{}", task.id, plan.id)) {
            crate::plan::fresh(ctx, task, plan)?;
            crate::plan_decompose::apply(ctx, &task.id, &plan.id)?;
        }
        return Ok(());
    }
    crate::plan::fresh(ctx, task, plan)?;
    let Some(review) = plan.reviews.last() else {
        if let Err(error) = crate::plan::expected_reviewer(&ctx.store.config, task) {
            return escalate(ctx, task, &format!("{error:#}"));
        }
        crate::plan_review(ctx, &task.id, None)?;
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
                        task,
                        "plan reviewer disappeared before recording a verdict",
                    )?;
                }
            }
        }
        Some(Verdict::Approve) => {
            if ctx
                .store
                .config
                .review_policy(task.project.as_deref(), task.review)
                .plan_accept
                == corral_core::Accept::Auto
            {
                crate::plan_review::accept_auto(ctx, &task.id, &plan.id)?;
            } else if run.is_none_or(|r| r.state != PlanRunState::AwaitingOwner) {
                ctx.store.update(&task.id, |t| {
                    if let Some(run) = t.plan_runs.last_mut() {
                        run.state = PlanRunState::AwaitingOwner;
                    }
                    t.status = Status::NeedsMe;
                    t.queued = false;
                    Ok(())
                })?;
                ctx.store.log_event(
                    Some(&task.id),
                    "plan_awaiting_owner",
                    json!({"target":"plan","plan":plan.id}),
                )?;
            }
        }
        Some(Verdict::Changes) => {
            let Some(run) = run else {
                return escalate(
                    ctx,
                    task,
                    "plan changes require a planner; dispatch a plan run",
                );
            };
            let max = ctx
                .store
                .config
                .review_policy(task.project.as_deref(), task.review)
                .max_rounds;
            if run.rounds >= max {
                return escalate(ctx, task, "plan review reached max_rounds");
            }
            plan_dispatch::revise(
                ctx,
                task,
                review
                    .notes
                    .as_deref()
                    .context("plan change verdict has no notes")?,
            )?;
        }
        Some(Verdict::Escalate) => escalate(
            ctx,
            task,
            review.notes.as_deref().unwrap_or("plan review escalated"),
        )?,
    }
    Ok(())
}

fn escalate(ctx: &Ctx, task: &Task, reason: &str) -> Result<()> {
    let mut escalated = false;
    ctx.store.update(&task.id, |t| {
        // A concurrent owner verdict or review retry supersedes this observation.
        if t.plans.last() != task.plans.last()
            || t.plan_runs.last().map(|r| &r.id) != task.plan_runs.last().map(|r| &r.id)
        {
            return Ok(());
        }
        if let Some(plan) = t.plans.last_mut() {
            crate::plan_review::terminalize_pending(plan, reason);
        }
        t.status = Status::NeedsMe;
        t.queued = false;
        if let Some(run) = t.plan_runs.last_mut() {
            run.state = PlanRunState::Escalated;
            run.pending_reply = Some(reason.into());
        }
        escalated = true;
        Ok(())
    })?;
    if !escalated {
        return Ok(());
    }
    ctx.store.log_event(
        Some(&task.id),
        "plan_escalated",
        json!({"target":"plan","reason":reason}),
    )
}
