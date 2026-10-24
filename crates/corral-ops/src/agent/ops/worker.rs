//! Handlers of the worker: wait, progress, checkpoint, evidence, artifact, gate and done ops.
//! Owner: T-252 child worker-role. `call.rs` routes these ops here; until the child lands every
//! one answers `op_unavailable` naming it, and `lands` reports none as implemented.
use anyhow::Result;

use super::super::call::{AgentOp, CallResponse};
use super::super::envelope::Envelope;
use crate::Ctx;

/// The T-252 child that lands these handlers.
pub const OWNER: &str = "worker-role";

/// Whether this module handles `op` yet; hello advertises only landed ops.
pub(crate) fn lands(_op: AgentOp) -> bool {
    false
}

pub(crate) fn call(_ctx: &Ctx, _envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    Err(super::unavailable(op, OWNER))
}
