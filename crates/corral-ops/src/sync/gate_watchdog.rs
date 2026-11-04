//! Gate admission and execution deliberately leave their worker idle.
use anyhow::Result;
use corral_core::{GateRecord, Run, TaskHead, agent::AgentRole};

pub(crate) fn pending(head: &TaskHead, run: &Run) -> Result<bool> {
    if run.gate.as_ref().is_some_and(GateRecord::holds_slot) {
        return Ok(true);
    }
    Ok(head.queued_agent_gates()?.iter().any(|receipt| {
        receipt.identity.role == AgentRole::Worker
            && receipt.identity.task.as_deref() == Some(head.id.as_str())
            && receipt.identity.run.as_deref() == Some(run.id.as_str())
    }))
}
