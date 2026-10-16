//! Pane access for an existing task run.
use anyhow::{Result, anyhow};

use crate::Ctx;

/// Focuses the current run's pane in herdr. Error when the task has no run with a herdr ref.
pub fn jump(ctx: &Ctx, id: &str) -> Result<()> {
    let task = ctx.store.get(id)?;
    let pane = task
        .current_run()
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
        .ok_or_else(|| anyhow!("{} has no run attached to herdr", task.id))?;
    ctx.herdr.pane_focus(&pane)
}

/// Last `lines` lines of the current run's pane (empty Vec when no live pane).
pub fn live_tail(ctx: &Ctx, id: &str, lines: usize) -> Result<Vec<String>> {
    let task = ctx.store.get(id)?;
    if matches!(
        task.status,
        corral_core::Status::Done | corral_core::Status::Dropped
    ) {
        return Ok(Vec::new());
    }
    let Some(pane) = task
        .current_run()
        .filter(|r| r.ended.is_none() && r.outcome.is_none())
        .and_then(|r| r.herdr.as_ref())
        .map(|h| h.pane_id.clone())
    else {
        return Ok(Vec::new());
    };
    if ctx.herdr.pane_get(&pane)?.is_none() {
        return Ok(Vec::new());
    }
    let Some(text) = ctx.herdr.pane_read_if_present(&pane, lines)? else {
        return Ok(Vec::new());
    };
    let all: Vec<String> = text.lines().map(str::to_string).collect();
    let skip = all.len().saturating_sub(lines);
    Ok(all.into_iter().skip(skip).collect())
}
