//! Durable worker wakes: one `wait_wake` per wait or gate transition, posted into the run's
//! inbox and marked fired in the same task transaction. Owner: T-252 child worker-role.
use anyhow::Result;

use super::effects::DueWake;
use crate::Ctx;

/// The T-252 child that lands wake posting.
pub const OWNER: &str = "worker-role";

/// Posts the wakes sync found due and returns how many were posted. Before worker-role lands
/// nothing is posted or marked fired, so every due wake stays due for the real adapter.
pub fn post_due(_ctx: &Ctx, _due: &[DueWake]) -> Result<usize> {
    Ok(0)
}
