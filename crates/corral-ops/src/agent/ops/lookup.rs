//! receipt_lookup: reads one receipt of the requester's own named container, whatever
//! generation committed it. Authentication only; nothing is written.
use anyhow::Result;
use corral_core::Store;
use corral_core::agent::{self, RequestReceipt};
use serde::Deserialize;

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode, Resolved};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::{self, Envelope};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LookupPayload {
    request_id: String,
    /// PM only: the target task whose `agent_pm_receipts` hold the receipt.
    #[serde(default)]
    task: Option<String>,
}

fn reject(code: RejectCode, reason: &str) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

/// A PM lookup naming a task reads that task's PM receipts, which are part of the PM
/// container, so the lookup's own request ID must not collide with one of them either.
fn pm_task_receipt(
    ctx: &Ctx,
    scope: &str,
    task: &str,
    envelope: &Envelope,
    request_id: &str,
) -> Result<Option<RequestReceipt>> {
    if Store::normalize_id(task).ok().as_deref() != Some(task) {
        return Err(reject(
            RejectCode::BadRequest,
            "payload.task must be a canonical task ID",
        ));
    }
    let task = match ctx.store.get(task) {
        Ok(task) => task,
        Err(error) if format!("{error}").ends_with("not found") => return Ok(None),
        Err(error) => return Err(error),
    };
    if task.project.as_deref() != Some(scope) {
        return Err(reject(
            RejectCode::OutOfScope,
            "the task is outside the PM's project scope",
        ));
    }
    if let Some(own) = task
        .agent_pm_receipts
        .iter()
        .find(|r| r.request_id == envelope.request_id)
    {
        admission::same_request(own, &envelope.op, &envelope.payload_sha256)?;
    }
    Ok(task
        .agent_pm_receipts
        .into_iter()
        .find(|r| r.request_id == request_id))
}

pub(crate) fn receipt_lookup(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: LookupPayload = super::payload(envelope, "request_id, task?")?;
    envelope::request_id_shape(&payload.request_id)?;
    let admitted = match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::New(admitted) => admitted,
        Admission::Replay(stored) => {
            let container = admission::container_of(&envelope.identity);
            return crate::agent::receipts::respond(ctx, &container, *stored, true);
        }
    };
    let container = &admitted.origin.binding.container;
    let found = match (&admitted.resolved, payload.task.as_deref()) {
        (Resolved::Task(task), None) => agent::container_state(task, container)
            .and_then(|state| state.receipt(&payload.request_id))
            .cloned(),
        (Resolved::Assistant(assistant, _), None) => {
            assistant.state.receipt(&payload.request_id).cloned()
        }
        (Resolved::Task(_) | Resolved::Assistant(..), Some(_)) => {
            return Err(reject(
                RejectCode::BadRequest,
                "payload.task applies only to PM lookups",
            ));
        }
        (Resolved::Pm(pm), None) => pm.state.receipt(&payload.request_id).cloned(),
        (Resolved::Pm(pm), Some(task)) => pm_task_receipt(
            ctx,
            &pm.state.binding.project,
            task,
            envelope,
            &payload.request_id,
        )?,
    };
    Ok(CallResponse::Lookup {
        request_id: admitted.origin.request_id.clone(),
        identity: admitted.origin.identity.clone(),
        target: payload.request_id,
        found: found.map(Box::new),
    })
}
