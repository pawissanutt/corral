//! Held cards keep their panes for inspection; only a gone or idle Backlog run is closed.
use std::collections::HashMap;

use anyhow::Result;
use corral_core::{RunOutcome, Status, TaskHead};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// A held card has relinquished dispatch authority. Preserve its pane for inspection;
/// membership moves remain permitted and do not themselves terminate a live worker.
pub(super) fn close_held_run(
    ctx: &Ctx,
    task: &TaskHead,
    panes: &HashMap<String, PaneInfo>,
    now: Timestamp,
) -> Result<()> {
    let _span = corral_core::timing::span("sync.close_held_run");
    if task.status != Status::Backlog || !task.dispatch_hold || task.park.is_some() {
        return Ok(());
    }
    let Some(run) = task
        .current_run
        .as_deref()
        .filter(|run| run.outcome.is_none())
    else {
        return Ok(());
    };
    let Some(pane_id) = run.herdr.as_ref().map(|h| &h.pane_id) else {
        return Ok(());
    };
    let reason = match panes.get(pane_id) {
        None => "gone",
        Some(pane) if matches!(pane.status, AgentStatus::Idle | AgentStatus::Done) => "idle",
        _ => return Ok(()),
    };
    let run_id = run.id.clone();
    let _attribution = ctx.store.attribute(corral_core::Actor::System);
    ctx.store.update_head(&task.id, |current| {
        if current.status == Status::Backlog
            && current.dispatch_hold
            && current.park.is_none()
            && let Some(run) = current
                .current_run_mut()
                .filter(|r| r.id == run_id && r.outcome.is_none())
        {
            run.outcome = Some(RunOutcome::Abandoned);
            run.ended = Some(now);
            ctx.store.log_event(
                Some(&task.id),
                "run_abandoned",
                json!({"run": run_id, "reason": reason}),
            )?;
        }
        Ok(())
    })?;
    Ok(())
}
