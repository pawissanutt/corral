//! Per-task operational holds and a workspace focus. No grant or worker-session changes.
use anyhow::{Result, ensure};
use corral_core::{Status, TaskHold};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Focus {
    pub task: String,
    pub all: bool,
    #[serde(default)]
    pub held: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Held(String);
impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Held {}

/// Fence tasks filed after the focus snapshot, before sync can persist their hold.
pub(crate) fn require_clear<T: corral_core::TaskFieldsRef + ?Sized>(
    store: &corral_core::Store,
    task: &T,
) -> Result<()> {
    if let Some(hold) = task.hold() {
        return Err(Held(format!("{}: {}", task.id(), hold.refusal())).into());
    }
    if let Some(focus) = store.record::<Focus>("task-focus", "current")? {
        let target = store.head(&focus.task)?;
        if !matches!(target.status, Status::Done | Status::Dropped)
            && task.priority() > &target.priority
            && (focus.all || task.project() == &target.project)
        {
            return Err(Held(format!(
                "{}: Held: focus {} ({})",
                task.id(),
                target.id,
                target.priority
            ))
            .into());
        }
    }
    Ok(())
}

pub fn hold(ctx: &Ctx, id: &str, reason: Option<&str>) -> Result<()> {
    let id = ctx.store.head(id)?.id.clone();
    set(ctx, &id, reason.unwrap_or("paused"), None)?;
    cancel_held(ctx)
}

fn set(ctx: &Ctx, id: &str, reason: &str, focus: Option<&str>) -> Result<()> {
    ctx.store.update_head(id, |task| {
        ensure!(
            !matches!(task.status, Status::Done | Status::Dropped),
            "{} is terminal",
            task.id
        );
        // Focus never takes ownership of an existing manual hold.
        if task.hold.is_some() && focus.is_some() {
            return Ok(());
        }
        let hold = TaskHold {
            reason: reason.into(),
            at: ctx.store.now(),
            resume: task
                .review_wait
                .or_else(|| task.hold.as_ref().and_then(|h| h.resume)),
            focus: focus.map(str::to_owned),
        };
        if task
            .hold
            .as_ref()
            .is_some_and(|h| h.reason == hold.reason && h.focus == hold.focus)
        {
            return Ok(());
        }
        task.hold = Some(hold.clone());
        if let Some(focus) = focus
            && let Some(mut record) = ctx
                .store
                .record::<Focus>("task-focus", "current")?
                .filter(|r| r.task == focus)
            && !record.held.contains(&task.id)
        {
            record.held.push(task.id.clone());
            ctx.store.save_record("task-focus", "current", &record)?;
        }
        task.review_wait = None;
        ctx.store
            .log_event(Some(&task.id), "held", json!({"hold": hold}))?;
        Ok(())
    })?;
    Ok(())
}

pub fn release(ctx: &Ctx, id: &str) -> Result<()> {
    let id = ctx.store.head(id)?.id.clone();
    release_from(ctx, &id, None)
}

fn release_from(ctx: &Ctx, id: &str, focus: Option<&str>) -> Result<()> {
    ctx.store.update_head(id, |task| {
        if task
            .hold
            .as_ref()
            .is_none_or(|h| focus.is_some() && h.focus.as_deref() != focus)
        {
            return Ok(());
        }
        let hold = task.hold.take();
        task.review_wait = hold.as_ref().and_then(|h| h.resume);
        if task
            .head()
            .current_run
            .as_ref()
            .and_then(|r| r.current_child.as_deref())
            .is_some_and(|c| c.outcome == Some(corral_core::RunOutcome::Cancelled))
        {
            task.review_wait = Some(corral_core::ReviewWork::Reviewer);
        }
        if task
            .head()
            .current_run
            .as_ref()
            .and_then(|r| r.gate.as_ref())
            .or_else(|| {
                task.head()
                    .last_aggregate_attempt
                    .as_ref()
                    .and_then(|a| a.gate.as_ref())
            })
            .is_some_and(|g| g.result == corral_core::GateResult::Cancelled)
            || !task.head().queued_agent_gates()?.is_empty()
            || crate::plan_acceptance::checkpoint::cancelled(&ctx.store, &task.id)?
        {
            task.review_wait = Some(corral_core::ReviewWork::Gate);
        }
        ctx.store
            .log_event(Some(&task.id), "released", json!({"hold": hold}))?;
        Ok(())
    })?;
    Ok(())
}

pub fn focus(ctx: &Ctx, id: Option<&str>, all: bool) -> Result<()> {
    let _lock = ctx
        .store
        .try_lock("task-focus")?
        .ok_or_else(|| anyhow::anyhow!("focus is being changed; retry"))?;
    let next = id.map(|id| ctx.store.head(id)).transpose()?;
    if let Some(task) = &next {
        ensure!(
            !matches!(task.status, Status::Done | Status::Dropped),
            "{} is terminal",
            task.id
        );
    }
    clear_focus(ctx)?;
    if let Some(task) = next {
        ctx.store.save_record(
            "task-focus",
            "current",
            &Focus {
                task: task.id.clone(),
                all,
                held: Vec::new(),
            },
        )?;
        release_from(ctx, &task.id, None)?;
        reconcile_focus(ctx)?;
    }
    cancel_held(ctx)
}

fn clear_focus(ctx: &Ctx) -> Result<()> {
    if let Some(focus) = ctx.store.record::<Focus>("task-focus", "current")? {
        for id in &focus.held {
            release_from(ctx, id, Some(&focus.task))?;
        }
        ctx.store.delete_record("task-focus", "current")?;
    }
    Ok(())
}

fn reconcile_focus(ctx: &Ctx) -> Result<()> {
    let Some(focus) = ctx.store.record::<Focus>("task-focus", "current")? else {
        return Ok(());
    };
    let target = ctx.store.head(&focus.task)?;
    if matches!(target.status, Status::Done | Status::Dropped) {
        return clear_focus(ctx);
    }
    for task in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        if task.priority > target.priority && (focus.all || task.project == target.project) {
            set(
                ctx,
                &task.id,
                &format!("focus {} ({})", target.id, target.priority),
                Some(&target.id),
            )?;
        } else {
            release_from(ctx, &task.id, Some(&target.id))?;
        }
    }
    Ok(())
}

pub(crate) fn reconcile(ctx: &Ctx) -> Result<()> {
    if let Some(_lock) = ctx.store.try_lock("task-focus")? {
        reconcile_focus(ctx)?;
    }
    cancel_held(ctx)
}

fn cancel_held(ctx: &Ctx) -> Result<()> {
    // A launch already submitting finishes its atomic send, then the next pass cancels it.
    // A launch still preparing is fenced by the durable hold at submission.
    let Some(_budget) = ctx.store.try_lock("review-budget")? else {
        return Ok(());
    };
    for task in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        if task.hold.is_some() {
            crate::review_budget::cancel_task(ctx, &task.id, "held")?;
        }
    }
    Ok(())
}

/// Cancelled explicit worker gates resume even when review autopilot is off. Native requests
/// have their own receipted queue. Automatic attempts still require current authority.
pub(crate) fn resume_gates(ctx: &Ctx) -> Result<()> {
    let tasks = ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?;
    for task in tasks {
        if task.hold.is_none()
            && task.review_wait == Some(corral_core::ReviewWork::Gate)
            && crate::plan_acceptance::checkpoint::cancelled(&ctx.store, &task.id)?
        {
            let outcome = if crate::plan_acceptance::checkpoint::automatic(&ctx.store, &task.id)? {
                crate::plan_acceptance::checkpoint::plan_checkpoint_automatic(ctx, &task.id)
                    .map(|_| ())
            } else {
                crate::plan_acceptance::checkpoint::plan_checkpoint(ctx, &task.id).map(|_| ())
            };
            match outcome {
                Ok(_) => {}
                Err(e)
                    if e.is::<crate::review_budget::CapacityWait>()
                        || e.is::<crate::review_budget::LaunchBusy>() => {}
                Err(e) => {
                    crate::messages::report(Some(&task.id), "checkpoint requeue", format!("{e:#}"))
                }
            }
            continue;
        }
        if task.hold.is_none()
            && task.review_wait == Some(corral_core::ReviewWork::Gate)
            && task
                .last_aggregate_attempt
                .as_ref()
                .and_then(|a| a.gate.as_ref())
                .is_some_and(|g| g.result == corral_core::GateResult::Cancelled)
        {
            match crate::plan_acceptance::plan_verify(ctx, &task.id) {
                Ok(_) => {}
                Err(e)
                    if e.is::<crate::review_budget::CapacityWait>()
                        || e.is::<crate::review_budget::LaunchBusy>() => {}
                Err(e) => {
                    crate::messages::report(Some(&task.id), "Plan gate requeue", format!("{e:#}"))
                }
            }
            continue;
        }
        if task.hold.is_some() || !task.queued_agent_gates()?.is_empty() {
            continue;
        }
        if task.review_wait == Some(corral_core::ReviewWork::Gate)
            && task
                .current_run
                .as_ref()
                .and_then(|r| r.gate.as_ref())
                .is_some_and(|g| g.result == corral_core::GateResult::Cancelled)
        {
            let gate = task.current_run.as_ref().and_then(|r| r.gate.as_ref());
            let automatic = prior_automatic(
                ctx,
                &task,
                corral_core::OperatingActionKind::Gate,
                gate.and_then(|g| g.identity.as_ref())
                    .map(|i| i.attempt.as_str()),
            )?;
            let outcome = if automatic {
                crate::gate::gate_start_automatic(ctx, &task.id)
            } else {
                crate::gate::gate_start(ctx, &task.id)
            };
            match outcome {
                Ok(_) => {}
                Err(e)
                    if e.is::<crate::review_budget::CapacityWait>()
                        || e.is::<crate::review_budget::LaunchBusy>() => {}
                Err(e) => crate::messages::report(Some(&task.id), "gate requeue", format!("{e:#}")),
            }
        }
        if task.review_wait == Some(corral_core::ReviewWork::Reviewer)
            && task.status == Status::Review
            && let Some(child) = task
                .current_run
                .as_ref()
                .and_then(|r| r.current_child.as_deref())
                .filter(|c| c.outcome == Some(corral_core::RunOutcome::Cancelled))
        {
            let automatic = prior_automatic(
                ctx,
                &task,
                corral_core::OperatingActionKind::Review,
                Some(&child.id),
            )?;
            let outcome = if automatic {
                crate::reviewer::review_run_automatic(ctx, &task.id, None)
            } else {
                crate::reviewer::review_run(
                    ctx,
                    &task.id,
                    child.review.as_ref().map(|r| r.reviewer.as_str()),
                )
            };
            if let Err(e) = outcome
                && !e.is::<crate::review_budget::CapacityWait>()
                && !e.is::<crate::review_budget::LaunchBusy>()
            {
                crate::messages::report(Some(&task.id), "review requeue", format!("{e:#}"));
            }
        }
    }
    Ok(())
}

fn prior_automatic(
    ctx: &Ctx,
    task: &corral_core::TaskHead,
    kind: corral_core::OperatingActionKind,
    attempt: Option<&str>,
) -> Result<bool> {
    let Some((run, attempt)) = task.current_run.as_ref().zip(attempt) else {
        return Ok(false);
    };
    Ok(crate::operating::automatic_driver(&crate::operating::load(
        ctx, &task.id, &run.id, kind, attempt,
    )?))
}

#[cfg(test)]
mod tests;
