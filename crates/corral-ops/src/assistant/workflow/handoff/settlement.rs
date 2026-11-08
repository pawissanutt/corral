//! Held ownership is necessary but not sufficient: settle verified native writers
//! before publishing Saved or enabling a returned local writer. Unknown legacy
//! writers remain a visible wait, never an inferred process exit.
use crate::Ctx;
use anyhow::{Result, ensure};
use corral_core::agent::{ContainerRef, HostHealth};
use corral_core::{RunOutcome, Status, TaskRef};

pub(super) fn settle(ctx: &Ctx, id: &str) -> Result<Vec<String>> {
    let task = ctx.store.get(id)?;
    ensure!(task.dispatch_hold, "handoff_stop: admission is not held");
    let mut waiting = Vec::new();
    if let Some(launch) = &task.launch {
        if launch.stage == corral_core::LaunchStage::Submitting {
            waiting.push("launch submission has not settled".into());
        } else {
            crate::launch::cancel(ctx, &task)?;
        }
    }
    if let Some(run) = task.current_run() {
        if run.outcome.is_none() && run.agent_host.is_none() {
            waiting.push(format!("worker {} has no verifiable native host", run.id));
        }
        if run.review.as_ref().is_some_and(|r| r.holds_slot())
            && run
                .review_runs
                .last()
                .is_none_or(|r| r.agent_host.is_none())
        {
            waiting.push(format!(
                "review {} has no dedicated verifiable native host",
                run.id
            ));
        }
        if run.gate.as_ref().is_some_and(|g| {
            g.result == corral_core::GateResult::Running
                && (g.identity.is_none() || !cfg!(target_os = "linux"))
        }) {
            waiting.push(format!("gate {} has no verifiable process scope", run.id));
        }
    }
    if let Some(run) = task.plan_runs.last().filter(|r| r.holds_slot())
        && run.agent_host.is_none()
    {
        waiting.push(format!("planner {} has no verifiable native host", run.id));
    }
    for (plan, review) in task.pending_plan_reviewers()? {
        if task
            .plan_review(&plan, &review.id)?
            .is_none_or(|r| r.agent_host.is_none())
        {
            waiting.push(format!(
                "plan review {} has no dedicated verifiable native host",
                review.id
            ));
        }
    }
    // A shared PM is never in this inventory and is never signalled. A legacy or
    // unconfirmed holder cannot be made safe by merely closing its pane.
    if !waiting.is_empty() {
        return Ok(waiting);
    }
    let mut containers = task.unfinished_native_containers()?;
    if let Some(run) = task.current_run().filter(|r| r.agent_host.is_some()) {
        let worker = ContainerRef::Worker {
            task: id.into(),
            run: run.id.clone(),
        };
        if !containers.contains(&worker) {
            containers.push(worker);
        }
    }
    let active = !containers.is_empty()
        || task.current_run().is_some_and(|r| {
            r.gate
                .as_ref()
                .is_some_and(|g| g.result == corral_core::GateResult::Running)
        });
    if !active {
        return Ok(Vec::new());
    }
    ensure!(
        crate::session_scope::owns_task(ctx, &task)?,
        "handoff_stop: selected session does not own the active task containers"
    );
    let _review = crate::review_budget::hold_guard(ctx, id)?;
    for container in &containers {
        match crate::agent::recovery::stop_for_handoff(ctx, container) {
            Ok(holders) => waiting.extend(
                holders
                    .into_iter()
                    .map(|h| format!("{}: {h}", container.key())),
            ),
            Err(error) => waiting.push(format!("{}: {error:#}", container.key())),
        }
    }
    if !waiting.is_empty() {
        return Ok(waiting);
    }
    // Normal cancellation owns review/gate accounting and deliberately protects PM
    // panes. It is invoked only after dedicated native writers/executors are gone.
    crate::review_budget::cancel_task(ctx, id, "co-work scope held")?;
    let fresh = ctx.store.get(id)?;
    if fresh.plan_runs.last().is_some_and(|r| r.holds_slot()) {
        crate::plan_lifecycle::plan_cancel(ctx, id)?;
    }
    ctx.store.update(id, |task| {
        if let Some(run) = task.current_run_mut().filter(|r| r.outcome.is_none()) {
            ensure!(
                run.agent_host
                    .as_ref()
                    .is_some_and(|h| h.binding.health == HostHealth::Fenced),
                "handoff_stop: worker binding changed before cancellation"
            );
            run.outcome = Some(RunOutcome::Cancelled);
            run.ended = Some(corral_core::clock::now());
            run.usage_pending = true;
            task.status = Status::NeedsMe;
        }
        task.dispatch_hold = true;
        task.queued = false;
        task.dispatch_plan = None;
        ctx.store.log_event(
            Some(id),
            "handoff_writers_settled",
            serde_json::json!({"containers":containers.len()}),
        )?;
        Ok(())
    })?;
    Ok(Vec::new())
}
