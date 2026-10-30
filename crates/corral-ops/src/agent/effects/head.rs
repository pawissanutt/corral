//! Final fences and receipt writes for task-backed swept operations.
use super::*;
use corral_core::{TaskHeadMut, TaskRef};

pub(crate) fn recheck_head(task: &impl TaskRef, origin: &RequestOrigin) -> Result<()> {
    if let Some(stored) = task.agent_receipt(&origin.binding.container, &origin.request_id)? {
        admission::same_request(&stored, &origin.op, &origin.payload_sha256)?;
        return Err(Replayed(Box::new(stored)).into());
    }
    admission::recheck_task_head(task, &origin.binding)
}

pub(crate) fn record_head(
    task: &mut TaskHeadMut,
    origin: &RequestOrigin,
    commit: Commit,
) -> Result<RequestReceipt> {
    let container = &origin.binding.container;
    let effect = commit
        .effect
        .and_then(|(kind, reference)| effect(kind, &reference));
    let mut receipt = new_receipt(origin, ReceiptState::Committed, commit.result, effect, None)?;
    receipt.post_pending = commit.owed.into_iter().map(String::from).collect();
    let host = task
        .agent_head(container)?
        .context("the agent binding disappeared")?;
    if host.records.receipt_count() >= RECEIPTS_MAX as u64 {
        return Err(reject(
            RejectCode::ReceiptLimit,
            format!("the container already holds {RECEIPTS_MAX} receipts"),
        ));
    }
    task.put_agent_record(container, agent::NativeRecord::Receipt(receipt.clone()))?;
    Ok(receipt)
}
