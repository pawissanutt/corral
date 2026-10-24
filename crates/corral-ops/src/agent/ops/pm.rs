//! Handlers of the PM: reply, comment, capture and the typed reads ops.
//! Owner: T-252 child pm-role. `call.rs` routes these ops here; until the child lands every
//! one answers `op_unavailable` naming it, and `lands` reports none as implemented.
use anyhow::Result;

use super::super::call::{AgentOp, CallResponse};
use super::super::envelope::Envelope;
use crate::Ctx;

/// The T-252 child that lands these handlers.
pub const OWNER: &str = "pm-role";

/// Whether this module handles `op` yet; hello advertises only landed ops.
pub(crate) fn lands(_op: AgentOp) -> bool {
    false
}

pub(crate) fn call(_ctx: &Ctx, _envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    Err(super::unavailable(op, OWNER))
}
