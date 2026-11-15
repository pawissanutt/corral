//! A failed first bind consumes the prepared launch, not another recovery attempt.
use super::{IntentPhase, PauseReason, intent_for, pause};
use crate::Ctx;
use anyhow::Result;
use corral_core::{Status, agent::ContainerRef};

/// Call only after detecting failure on a potentially paid native attempt. The keyed
/// intent lookup is enough to exclude ordinary starts; only a Prepared intent writes.
/// Returning true also keeps subsequent watchdog passes out of legacy terminalization,
/// which would replace the durable Paused reason with a synthetic result or Closed.
pub(crate) fn failed_start(ctx: &Ctx, container: &ContainerRef, generation: u32) -> Result<bool> {
    let Some(intent) = intent_for(&ctx.store, container, generation)? else {
        return Ok(false);
    };
    if intent.phase == IntentPhase::Paused {
        return Ok(true);
    }
    if intent.phase != IntentPhase::Prepared {
        return Ok(false);
    }
    // Pause owns the successor even when its native state has not been created.
    pause(
        ctx,
        container,
        PauseReason::SuccessorAmbiguous,
        "successor host lost before healthy bind",
    )?;
    if let Some(task) = container.task() {
        ctx.store.try_update_head(task, |head| {
            if head.status == Status::NeedsMe {
                return Ok(false);
            }
            head.status = Status::NeedsMe;
            Ok(true)
        })?;
    }
    Ok(true)
}
