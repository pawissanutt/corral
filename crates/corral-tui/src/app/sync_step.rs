//! One background update of the board: sync, then the live cost refresh of unfinished runs.

use corral_ops::{Change, Ctx};

pub(crate) fn run(ctx: &Ctx) -> Result<Vec<Change>, String> {
    let synced = corral_ops::sync(ctx).map_err(|e| corral_herdr::friendly_error(&e));
    corral_ops::refresh_active_usage(&ctx.store);
    synced
}

#[cfg(test)]
#[path = "sync_step_tests.rs"]
mod tests;
