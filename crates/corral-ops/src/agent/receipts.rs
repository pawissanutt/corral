//! Single-phase receipts: each op writes its receipt in its own final transaction, which first
//! rereads the container, returns an identical stored receipt, and repeats the liveness, fence
//! and saved-pane check. A refusal there writes nothing, so after a fence commits a missing
//! single-phase receipt is final. No wrapper holds a lock around a public operation.
use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::agent::{
    self, AGENT_RECORD_VERSION, AgentRunState, ClosureReason, ContainerRef, RECEIPTS_MAX,
    RESULT_MAX_BYTES, ReceiptEffect, ReceiptState, RequestOrigin, RequestReceipt, paths, pm_state,
    rejections,
};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::admission::{self, Admitted};
use super::call::{CallResponse, RejectCode, Rejection};
use crate::Ctx;

/// What an op's apply step contributes to its committed receipt.
pub(crate) struct Commit {
    pub result: Value,
    pub effect: Option<ReceiptEffect>,
    pub post_pending: Vec<String>,
}

#[cfg(test)]
type FailpointHook = Option<Box<dyn FnMut(&str)>>;

#[cfg(test)]
thread_local! {
    pub(crate) static FAILPOINT: std::cell::RefCell<FailpointHook> =
        const { std::cell::RefCell::new(None) };
}

/// Test-only crash and race points; a no-op in shipped builds.
pub(crate) fn failpoint(name: &str) {
    #[cfg(test)]
    FAILPOINT.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(name);
        }
    });
    #[cfg(not(test))]
    let _ = name;
}

fn append(
    state: &mut AgentRunState,
    origin: &RequestOrigin,
    commit: Commit,
) -> Result<RequestReceipt> {
    if state.receipts.len() >= RECEIPTS_MAX {
        return Err(Rejection::new(
            RejectCode::ReceiptLimit,
            format!("the container already holds {RECEIPTS_MAX} receipts"),
        )
        .into());
    }
    let bytes = serde_json::to_vec(&commit.result)?.len();
    anyhow::ensure!(
        bytes <= RESULT_MAX_BYTES,
        "receipt result of {bytes} bytes exceeds {RESULT_MAX_BYTES}"
    );
    let now = Timestamp::now();
    let receipt = RequestReceipt {
        version: AGENT_RECORD_VERSION,
        request_id: origin.request_id.clone(),
        op: origin.op.clone(),
        payload_sha256: origin.payload_sha256.clone(),
        identity: origin.identity.clone(),
        state: ReceiptState::Committed,
        result: commit.result,
        executor: None,
        effect: commit.effect,
        evidence: Vec::new(),
        post_pending: commit.post_pending,
        at: now,
        committed_at: Some(now),
    };
    state.receipts.push(receipt.clone());
    Ok(receipt)
}

/// Commits `apply` and its receipt in one transaction of the named container. `apply` sees the
/// container's closure reason, read in the same transaction.
pub(crate) fn commit(
    ctx: &Ctx,
    admitted: &Admitted,
    apply: impl FnOnce(&mut AgentRunState, Option<ClosureReason>) -> Result<Commit>,
) -> Result<CallResponse> {
    failpoint("agent_admitted_before_commit");
    let origin = &admitted.origin;
    let container = &origin.binding.container;
    let mut outcome: Option<(RequestReceipt, bool)> = None;
    match container.task() {
        None => pm_state::update(&ctx.store, |pm| {
            let pm = pm.as_mut().ok_or_else(|| {
                Rejection::new(RejectCode::ContainerNotFound, "the PM binding disappeared")
            })?;
            if let Some(stored) = pm.state.receipt(&origin.request_id) {
                admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
                outcome = Some((stored.clone(), true));
                return Ok(((), false));
            }
            admission::recheck_pm(pm, origin.binding.generation)?;
            let commit = apply(&mut pm.state, None)?;
            outcome = Some((append(&mut pm.state, origin, commit)?, false));
            Ok(((), true))
        })?,
        Some(task_id) => {
            ctx.store.try_update(task_id, |task| {
                let state = agent::container_state(task, container).ok_or_else(|| {
                    Rejection::new(
                        RejectCode::ContainerNotFound,
                        "the agent binding disappeared",
                    )
                })?;
                if let Some(stored) = state.receipt(&origin.request_id) {
                    admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
                    outcome = Some((stored.clone(), true));
                    return Ok(false);
                }
                admission::recheck_task(task, &origin.binding)?;
                let closure = agent::container_closure(task, container);
                let state = agent::container_state_mut(task, container)
                    .context("the agent binding disappeared")?;
                let commit = apply(state, closure)?;
                outcome = Some((append(state, origin, commit)?, false));
                Ok(true)
            })?;
        }
    }
    let (receipt, replayed) = outcome.context("the commit produced no receipt")?;
    respond(ctx, container, receipt, replayed)
}

/// Completes owed post-commit effects, then answers with the stored receipt.
pub(crate) fn respond(
    ctx: &Ctx,
    container: &ContainerRef,
    mut receipt: RequestReceipt,
    replayed: bool,
) -> Result<CallResponse> {
    complete_post_pending(&ctx.store, container, &mut receipt)?;
    Ok(CallResponse::Receipt { receipt, replayed })
}

fn event_detail(container: &ContainerRef, receipt: &RequestReceipt) -> Value {
    let mut detail = json!({
        "request_id": receipt.request_id, "op": receipt.op, "container": container.key(),
        "role": receipt.identity.role.as_str(), "generation": receipt.identity.generation,
    });
    for key in ["kind", "turn", "class"] {
        if let Some(value) = receipt.result.get(key) {
            detail[key] = value.clone();
        }
    }
    detail
}

/// Event lines are written after the commit and may repeat after a crash; each carries the
/// request ID so readers count it once. The flag clears only after the line is written.
fn complete_post_pending(
    store: &Store,
    container: &ContainerRef,
    receipt: &mut RequestReceipt,
) -> Result<()> {
    let events: Vec<String> = receipt
        .post_pending
        .iter()
        .filter(|entry| entry.starts_with("event:"))
        .cloned()
        .collect();
    if events.is_empty() {
        return Ok(());
    }
    for entry in &events {
        let kind = entry.trim_start_matches("event:");
        store.log_event(container.task(), kind, event_detail(container, receipt))?;
    }
    let clear = |state: &mut AgentRunState| {
        let stored = state
            .receipts
            .iter_mut()
            .find(|r| r.request_id == receipt.request_id);
        stored.is_some_and(|stored| {
            let before = stored.post_pending.len();
            stored.post_pending.retain(|entry| !events.contains(entry));
            stored.post_pending.len() != before
        })
    };
    match container.task() {
        None => pm_state::update(store, |pm| {
            let changed = pm.as_mut().is_some_and(|pm| clear(&mut pm.state));
            Ok(((), changed))
        })?,
        Some(task_id) => {
            store.try_update(task_id, |task| {
                Ok(agent::container_state_mut(task, container).is_some_and(clear))
            })?;
        }
    }
    receipt.post_pending.retain(|entry| !events.contains(entry));
    Ok(())
}

/// Records one `agent_request_rejected` line per distinct refused request: an identical retry
/// (same request, container and refusal) is not recorded again. Reasons and identities carry
/// no secret: the envelope's auth block is never part of a response, a key or an event.
pub(crate) fn log_rejection(ctx: &Ctx, response: &CallResponse, input: &[u8]) -> Result<()> {
    let CallResponse::Rejected {
        rejection,
        op,
        request_id,
        identity,
    } = response
    else {
        return Ok(());
    };
    let task = identity
        .as_ref()
        .and_then(|i| i.task.as_deref())
        .filter(|t| Store::normalize_id(t).ok().as_deref() == Some(*t));
    let container = identity.as_ref().map(|i| admission::container_of(i).key());
    let detail = json!({
        "request_id": request_id, "op": op, "code": rejection.code.as_str(),
        "reason": rejection.reason, "container": container,
        "role": identity.as_ref().map(|i| i.role.as_str()),
        "generation": identity.as_ref().map(|i| i.generation),
    });
    let identified = request_id.is_some() && identity.is_some();
    let mut key = json!({
        "request_id": request_id, "op": op, "code": rejection.code.as_str(),
        "container": container, "role": detail["role"], "generation": detail["generation"],
    });
    if !identified {
        key["reason"] = json!(rejection.reason);
        key["input_sha256"] = json!(paths::sha256_hex(input));
    }
    let key = paths::sha256_hex(super::envelope::canonical_json(&key).as_bytes());
    rejections::log_rejection_once(&ctx.store, task, &key, detail).map(|_| ())
}
