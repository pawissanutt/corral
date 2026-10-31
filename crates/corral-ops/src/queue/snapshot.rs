//! Dispatch read-set guards shared by full-output and actual-head callers.
use crate::Ctx;
use anyhow::{Result, bail};
use corral_core::{Store, Task, TaskHead};

pub(crate) fn check_snapshot(
    ctx: &Ctx,
    store: &Store,
    expected: &Task,
    current: &Task,
) -> Result<()> {
    corral_core::Park::require_unparked(current)?;
    crate::tracker::check_action(current, "dispatch tracker")?;
    if expected != current || ctx.store.config != store.config {
        bail!(
            "{} or dispatch configuration changed; retry with the current state",
            current.id
        );
    }
    Ok(())
}

/// Any committed change of the task advances its revision.
pub(crate) fn check_snapshot_head(
    ctx: &Ctx,
    store: &Store,
    expected: &TaskHead,
    current: &TaskHead,
) -> Result<()> {
    crate::tracker::check_action(current, "dispatch tracker")?;
    if expected != current || ctx.store.config != store.config {
        bail!(
            "{} or dispatch configuration changed; retry with the current state",
            current.id
        );
    }
    Ok(())
}
