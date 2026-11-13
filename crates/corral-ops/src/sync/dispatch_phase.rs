//! Rows 28-45 of a sync pass: post-land hooks, gates, plan and review autopilots, admission,
//! attention, the PM event feed and cleanup. The pass holds no inventory lease here:
//! launching queued or autopilot work acquires its own exclusive ownership claim.
use anyhow::Result;

use super::{Change, PassInputs, StepId, gated, open_heads, reconcile_attention};
use crate::Ctx;
use crate::attention::AttentionBoard;

pub(super) fn dispatch(
    ctx: &Ctx,
    pass: &mut PassInputs<'_>,
    rebuild: &mut bool,
    attention: Option<&mut Option<AttentionBoard>>,
    search_stamp: &str,
    held_admission: Option<&str>,
) -> Result<Vec<Change>> {
    let _phase = corral_core::timing::span("sync.dispatch");
    let (now, scope) = (pass.now, pass.scope);
    let mut changes = Vec::new();
    // Post-land hooks run commands that may call corral: never under storage or the lease (T-509).
    gated(ctx, pass.gate(), StepId::PostLand, || {
        scope.workspace_only(|| crate::hooks::reconcile_post_land(ctx))
    })?;
    gated(ctx, pass.gate(), StepId::HandoffRecovery, || {
        scope.workspace_only(|| crate::assistant::workflow::handoff::recovery::reconcile(ctx))
    })?;
    gated(ctx, pass.gate(), StepId::ResumeGates, || {
        scope.workspace_only(|| crate::hold::resume_gates(ctx))
    })?;
    gated(ctx, pass.gate(), StepId::GateQueue, || {
        let native = scope.heads(ctx.store.unfinished_native_heads()?);
        crate::agent::gate_queue::advance_heads(ctx, &native)
    })?;
    crate::plan_pilot::advance_tasks(ctx, now, scope, pass.gate())?;
    changes.extend(gated(ctx, pass.gate(), StepId::Availability, || {
        scope.workspace_only(|| crate::availability::advance_all(ctx, now))
    })?);
    changes.extend(crate::autopilot::advance_tasks(
        ctx,
        now,
        scope,
        pass.gate(),
    )?);
    gated(ctx, pass.gate(), StepId::HoldSecond, || {
        crate::hold::reconcile(ctx, scope)
    })?;
    gated(ctx, pass.gate(), StepId::Movement, || {
        scope.workspace_only(|| crate::movement::reconcile(ctx, now, true))
    })?;
    // Its due-index query is its own due check, so it runs every pass.
    changes.extend(scope.workspace_only(|| crate::dashboard::advance(ctx, now))?);
    changes.extend(gated(ctx, pass.gate(), StepId::ReacquireWoken, || {
        crate::parked::reacquire_woken(ctx, &scope.heads(open_heads(ctx)?))
    })?);
    changes.extend(gated(ctx, pass.gate(), StepId::StartQueued, || {
        crate::queue_drain::start_queued(ctx, now, scope)
    })?);
    changes.extend(gated(ctx, pass.gate(), StepId::DispatchPolicy, || {
        crate::dispatch_policy::advance(ctx, now, scope)
    })?);
    gated(ctx, pass.gate(), StepId::Attention, || {
        scope.workspace_only(|| {
            let pm_live = Some(crate::pm::live_pm_pane(ctx)?.is_some());
            reconcile_attention(ctx, now, pm_live, attention, held_admission)
        })
    })?;
    crate::agent::effects::failpoint("sync_events");
    gated(ctx, pass.gate(), StepId::PmEvents, || {
        crate::pm_events::push_events(ctx, true)
    })?;
    gated(ctx, pass.gate(), StepId::DesktopSecond, || {
        crate::desktop_notifications::reconcile(ctx)
    })?;
    match crate::content_refresh::refresh_after_sync_if_due(ctx, &[], search_stamp) {
        Ok(followed) => *rebuild |= !followed,
        Err(error) => crate::content_refresh::defer_corrupt_blob_cache_refresh(error)?,
    }
    // External container IPC follows admission and holds no inventory lease.
    gated(ctx, pass.gate(), StepId::SandboxSweep, || {
        if let Err(error) = crate::sandbox_cleanup::sweep(ctx) {
            crate::messages::report(None, "sandbox-cleanup", format!("{error:#}"));
        }
        Ok(())
    })?;
    Ok(changes)
}
