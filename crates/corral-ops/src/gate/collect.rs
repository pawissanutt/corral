//! Waiting on a run's gate and collecting finished gates in sync.
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use corral_core::{GateRecord, GateResult};
use jiff::Timestamp;

use super::{LegacyGate, gate_collect, gate_collect_at};
use crate::Ctx;

/// Blocks until the current run's running gate finishes (polling its result file), then records it. Error when
/// the gate pane disappears before a result is written.
pub fn gate_wait(ctx: &Ctx, id: &str) -> Result<GateRecord> {
    loop {
        if let Some(record) = gate_collect(ctx, id)? {
            return Ok(record);
        }
        let task = ctx.store.get(id)?;
        let gate = task
            .current_run()
            .and_then(|r| r.gate.as_ref())
            .context("task has no running gate")?;
        let pane = gate.pane.as_deref();
        let listed = crate::pane_liveness::inventory(ctx);
        if listed.is_some()
            && crate::pane_liveness::gone(
                ctx,
                crate::pane_liveness::Sample {
                    task: (&task).into(),
                    pane,
                    started: gate.at,
                    seen: false,
                    present: pane.is_some_and(|id| {
                        listed.as_ref().is_some_and(|inventory| {
                            inventory.panes.iter().any(|p| p.pane_id == id)
                        })
                    }),
                    at: corral_core::clock::now(),
                    ticket: listed.as_ref().map(|inventory| inventory.ticket),
                    count_miss: true,
                },
            )?
        {
            // The script may have finished just before the pane closed.
            if let Some(record) = gate_collect(ctx, id)? {
                return Ok(record);
            }
            bail!(
                "{}: gate pane {} closed before the gate finished",
                task.id,
                pane.unwrap_or("?")
            );
        }
        drop(listed);
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Collects finished gates of nonterminal tasks (called by sync). Legacy gates cannot authorize a pass
/// and are skipped with one summary diagnostic. Returns the task ids whose gate was recorded.
pub(crate) fn collect_finished(ctx: &Ctx, now: Timestamp) -> Result<Vec<String>> {
    let mut done = Vec::new();
    let mut legacy = 0;
    for task in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        if !crate::session_scope::owns_head(ctx, &task)? {
            continue;
        }
        let running = task
            .current_run
            .as_ref()
            .and_then(|r| r.gate.as_ref())
            .is_some_and(|g| g.result == GateResult::Running);
        if !running {
            continue;
        }
        match gate_collect_at(ctx, &task.id, now) {
            Ok(Some(_)) => done.push(task.id.clone()),
            Ok(None) => {}
            Err(e) if e.is::<LegacyGate>() => legacy += 1,
            Err(e) => crate::messages::report(
                Some(&task.id),
                "gate",
                format!("collecting gate of {} failed: {e:#}", task.id),
            ),
        }
    }
    if legacy > 0 {
        crate::messages::report(
            None,
            "gate",
            format!(
                "skipped {legacy} legacy gate record(s); start new gates to verify active tasks"
            ),
        );
    }
    Ok(done)
}
