//! Stop-only half of recovery for an already-held C3 scope. Never bootstrap a
//! replacement, fabricate a turn end, or stop the shared PM/assistant container.
use super::*;
use corral_core::agent::{JobState, container_state_mut};

pub(super) fn require_host_start(ctx: &Ctx, container: &ContainerRef) -> Result<()> {
    if let Some(id) = container.task() {
        crate::assistant::workflow::handoff::admission::require_host_start(
            &ctx.store,
            &ctx.store.head(id)?,
            container,
        )?;
    }
    Ok(())
}

pub(crate) fn stop_for_handoff(ctx: &Ctx, container: &ContainerRef) -> Result<Vec<String>> {
    let id = container
        .task()
        .context("handoff_stop: only task containers can be stopped")?;
    let task = ctx.store.head(id)?;
    ensure!(
        task.dispatch_hold,
        "handoff_stop: hold admission before stopping a writer"
    );
    let journal = crate::assistant::workflow::handoff::lifecycle::for_task(&ctx.store, &task)?
        .context("handoff_stop: no transfer lineage")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    ensure!(
        matches!(
            transfer.disposition,
            crate::assistant::workflow::handoff::lifecycle::Disposition::Saving
                | crate::assistant::workflow::handoff::lifecycle::Disposition::Saved
                | crate::assistant::workflow::handoff::lifecycle::Disposition::TakingBack
                | crate::assistant::workflow::handoff::lifecycle::Disposition::CheckpointingPlan
        ),
        "handoff_stop: no pending stop instruction"
    );
    if transfer.disposition
        == crate::assistant::workflow::handoff::lifecycle::Disposition::CheckpointingPlan
    {
        crate::assistant::workflow::handoff::plan_checkpoint::require(&ctx.store, &task, &journal)?;
    }
    let (state, _) = snapshot(ctx, container)?;
    let n = state.binding.generation;
    ensure!(
        n > 0
            && state
                .current_generation()
                .is_some_and(|g| g.bound.is_some()),
        "handoff_stop: native launch identity is unconfirmed; keep the scope held"
    );
    // Stop even previously fenced hosts: a logical fence is not process exit.
    for generation in &state.generations {
        if let Some(bound) = &generation.bound {
            let host = Executor {
                pid: bound.pid,
                start_time: bound.start_time.clone(),
            };
            if !gone(&host)? {
                stop_host(&host)?;
            }
            ensure!(gone(&host)?, "handoff_stop: host is still alive");
        }
        fence_generation(&ctx.store, container, generation.n, "handoff_stop")?;
        remove_control(ctx, container, generation.n)?;
    }
    let deadline = Instant::now() + Duration::from_secs(ctx.store.config.agent.drain_secs);
    let mut waiting = Vec::new();
    for generation in &state.generations {
        waiting.extend(
            drain(ctx, container, generation.n, deadline)?
                .into_iter()
                .map(|request| format!("effect executor {request}")),
        );
    }
    if !waiting.is_empty() {
        return Ok(waiting);
    }
    reconcile(ctx, container, n)?;
    // Do not let the normal reaper's logical Lost/Reaped state hide a live group.
    // Retained job identities, including already-swept ones, are checked afresh.
    let (fresh, _) = snapshot(ctx, container)?;
    ensure!(
        fresh.binding.generation == n,
        "handoff_stop: generation changed; retry"
    );
    for receipt in &fresh.receipts {
        if matches!(
            receipt.state,
            ReceiptState::Reserved | ReceiptState::Uncertain
        ) {
            waiting.push(format!("unsettled effect {}", receipt.request_id));
        }
    }
    for job in &fresh.jobs {
        if !super::super::jobs::stop_for_handoff(job)? {
            waiting.push(format!("unverified job group {}", job.id));
        }
    }
    if !waiting.is_empty() {
        return Ok(waiting);
    }
    ctx.store.update(id, |task| {
        let state = container_state_mut(task, container).context("agent binding disappeared")?;
        ensure!(
            state.binding.generation == n,
            "handoff_stop: generation changed; retry"
        );
        for job in &mut state.jobs {
            if job.state.live() {
                job.ended_at = Some(corral_core::clock::now());
            }
            if job.state != JobState::Exited {
                job.state = JobState::Reaped;
            }
            job.group_clear = true;
            job.swept = true;
            job.detail = Some("Scope transfer observed the owned process group empty.".into());
        }
        let jobs = state.jobs.clone();
        if let ContainerRef::Worker { run, .. } = container {
            let run = task
                .runs
                .iter_mut()
                .find(|r| r.id == *run)
                .context("worker disappeared")?;
            for job in &jobs {
                super::super::jobs::resolve_ended(
                    run,
                    job,
                    corral_core::ReceiptOutcome::Cancelled,
                    corral_core::clock::now(),
                );
            }
        }
        ctx.store.log_event(
            Some(id),
            "handoff_host_stopped",
            json!({
                "container":container.key(), "generation":n, "jobs":jobs.len(), "replacement":false
            }),
        )?;
        Ok(())
    })?;
    // Lost turns intentionally remain historical open turns of fenced generations.
    // They are not authenticated turn_ended reports and must not be rewritten as such.
    Ok(Vec::new())
}
