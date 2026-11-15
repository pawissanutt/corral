//! Stop-only recovery shared by scope handoff and owner Terminate. No replacement
//! or authenticated turn-end is fabricated.
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
    let waiting = stop_container(ctx, container, false)?;
    if waiting.is_empty() {
        ctx.store.log_event(
            Some(id),
            "handoff_host_stopped",
            json!({"container":container.key(), "generation":n, "replacement":false}),
        )?;
    }
    Ok(waiting)
}

/// Terminate first asks the bridge to exit, then uses the same verified host,
/// reservation and job-group settlement as scope handoff.
pub(crate) fn stop_container(
    ctx: &Ctx,
    container: &ContainerRef,
    graceful: bool,
) -> Result<Vec<String>> {
    let (state, _) = snapshot(ctx, container)?;
    let n = state.binding.generation;
    let deadline = Instant::now() + Duration::from_secs(ctx.store.config.agent.drain_secs);
    if graceful {
        // Revoke mutation authority before asking the host to exit. Its bridge
        // credential remains available until shutdown has been requested.
        for generation in &state.generations {
            fence_generation(&ctx.store, container, generation.n, "owner_stop")?;
        }
    }
    if graceful && n > 0 {
        // A dead/unresponsive bridge cannot veto the owner's durable stop intent.
        // Verified process shutdown below is still mandatory before success.
        if let Ok(mut bridge) = BridgeClient::connect(&ctx.store, &recipient(container), n) {
            bridge.set_io_timeout(Duration::from_secs(
                ctx.store.config.agent.drain_secs.max(1),
            ))?;
            let _ = bridge.shutdown();
        }
        if let Some(bound) = state.current_generation().and_then(|g| g.bound.as_ref()) {
            let host = Executor {
                pid: bound.pid,
                start_time: bound.start_time.clone(),
            };
            wait_gone(&host, deadline)?;
        }
    }
    // Stop even previously fenced hosts: a logical fence is not process exit.
    let mut waiting = Vec::new();
    for generation in &state.generations {
        if let Some(bound) = &generation.bound {
            let host = Executor {
                pid: bound.pid,
                start_time: bound.start_time.clone(),
            };
            if !gone(&host)?
                && let Err(error) = stop_host(&host)
            {
                waiting.push(format!("host {}: {error:#}", host.pid));
            }
            if !gone(&host)? {
                waiting.push(format!("host {} is still alive", host.pid));
            }
        }
        fence_generation(
            &ctx.store,
            container,
            generation.n,
            if graceful {
                "owner_stop"
            } else {
                "handoff_stop"
            },
        )?;
        remove_control(ctx, container, generation.n)?;
    }
    let deadline = Instant::now() + Duration::from_secs(ctx.store.config.agent.drain_secs);
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
    for receipt in receipts(ctx, container)? {
        if matches!(
            receipt.state,
            ReceiptState::Reserved | ReceiptState::Uncertain
        ) {
            waiting.push(format!("unsettled effect {}", receipt.request_id));
        }
    }
    for job in &fresh.jobs {
        loop {
            if super::super::jobs::stop_for_handoff(job)? {
                break;
            }
            if Instant::now() >= deadline {
                waiting.push(format!("unverified job group {}", job.id));
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if !waiting.is_empty() {
        return Ok(waiting);
    }
    let settle_jobs = |state: &mut AgentRunState| -> Result<()> {
        ensure!(
            state.binding.generation == n,
            "conflict: generation changed while stopping"
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
            job.detail = Some("Stop observed the owned process group empty.".into());
        }
        Ok(())
    };
    if let Some(id) = container.task() {
        ctx.store.update(id, |task| {
            let state =
                container_state_mut(task, container).context("agent binding disappeared")?;
            settle_jobs(state)?;
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
            Ok(())
        })?;
    } else {
        pm_state::update(&ctx.store, |pm| {
            settle_jobs(&mut pm.as_mut().context("PM binding disappeared")?.state)?;
            Ok(((), true))
        })?;
    }
    // Lost turns intentionally remain historical open turns of fenced generations.
    // They are not authenticated turn_ended reports and must not be rewritten as such.
    Ok(Vec::new())
}
