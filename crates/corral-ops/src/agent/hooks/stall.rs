//! Provider-stall seam. A pi-native host reports provider errors as `agent_provider_error`
//! events that feed provider_stall directly, so no screen or session-log scan runs for it.
//! Owner: T-252 child recovery.
use anyhow::Result;
use corral_core::{Run, Task};
use jiff::Timestamp;

use super::consulted;
use crate::provider_stall::Drive;
use crate::{Change, Ctx};

pub(crate) fn advance_agent(
    _ctx: &Ctx,
    _task: &Task,
    _run: &Run,
    _now: Timestamp,
) -> Result<Option<Change>> {
    consulted("stall");
    Ok(None)
}

pub(crate) fn advance_reviewer(_ctx: &Ctx, _task: &str, _child: &Run) -> Result<Drive> {
    consulted("stall");
    Ok(Drive::Quiet)
}
