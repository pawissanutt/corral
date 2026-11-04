//! Detached inputs for sync phases that perform external work.
use anyhow::Result;
use corral_core::{Run, TaskHead, backend::HeadQuery};

use crate::Ctx;

/// The open board's scalar projections; slow per-task work opens its own keyed view.
pub(super) fn open_heads(ctx: &Ctx) -> Result<Vec<TaskHead>> {
    ctx.read_step(|read| {
        Ok(read
            .store
            .heads(&HeadQuery::Open { project: None })?
            .into_iter()
            .map(TaskHead::detach)
            .collect())
    })
}

/// Materialize the current native generation before process discovery can spend its lease.
pub(super) fn watchdog_inputs(ctx: &Ctx, head: &TaskHead, run: &Run) -> Result<Option<Run>> {
    if run.agent_host.is_none() {
        return Ok(Some(run.clone()));
    }
    ctx.read_step(|read| {
        let fresh = read.store.head(&head.id)?;
        if fresh.rev != head.rev {
            return Ok(None);
        }
        crate::agent::hooks::sync::watchdog_run(&fresh, run).map(Some)
    })
}
