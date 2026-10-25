//! Cancellation is confirmed by the host's authenticated turn end or its control reply.
//! Missing, negative or mismatched replies never retract either source's confirmation.
use anyhow::{Context, Result};
use corral_core::agent::{self, ContainerRef, TurnClass};

use crate::Ctx;

/// Records a confirmed cancellation when its turn exists; later turn reports carry their own
/// cancellation. Reclassification retains completed-report precedence.
pub(crate) fn confirm(ctx: &Ctx, container: &ContainerRef, n: u32, seq: u64) -> Result<()> {
    let Some(task_id) = container.task() else {
        return Ok(());
    };
    ctx.store.try_update(task_id, |task| {
        let closure = agent::container_closure(task, container);
        let state =
            agent::container_state_mut(task, container).context("the agent binding disappeared")?;
        let Some(index) = state
            .turns
            .iter()
            .position(|t| t.generation == n && t.seq == seq && t.started.is_some())
        else {
            return Ok(false);
        };
        let before = state.turns[index].class;
        state.turns[index].class = Some(TurnClass::Cancelled);
        if state.turns[index].ended.is_some() {
            super::super::turns::classify_ended(state, closure, index);
        }
        Ok(state.turns[index].class != before)
    })?;
    Ok(())
}
