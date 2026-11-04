//! Single-phase receipts: each op writes its receipt in its own final transaction, which first
//! rereads the container, returns an identical stored receipt, and repeats the liveness, fence
//! and saved-pane check. A refusal there writes nothing, so after a fence commits a missing
//! single-phase receipt is final. No wrapper holds a lock around a public operation.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, AGENT_RECORD_VERSION, AgentRunState, ClosureReason, ContainerRef, RECEIPTS_MAX,
    RESULT_MAX_BYTES, ReceiptEffect, ReceiptState, RequestOrigin, RequestReceipt, paths, pm_state,
    rejections,
};
use corral_core::assistant::AssistantContainer;
use corral_core::{Store, TaskRef};
use serde_json::{Value, json};

use super::admission::{self, Admitted};
use super::call::{CallResponse, RejectCode, Rejection};
use super::envelope::Envelope;
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
    let receipt = new_receipt(origin, commit)?;
    state.receipts.push(receipt.clone());
    Ok(receipt)
}

pub(crate) fn pm_capacity(store: &Store) -> Result<()> {
    if pm_state::child_count(store, "receipts")? >= RECEIPTS_MAX as u64 {
        return Err(Rejection::new(
            RejectCode::ReceiptLimit,
            format!("the container already holds {RECEIPTS_MAX} receipts"),
        )
        .into());
    }
    Ok(())
}

fn new_receipt(origin: &RequestOrigin, commit: Commit) -> Result<RequestReceipt> {
    let bytes = serde_json::to_vec(&commit.result)?.len();
    anyhow::ensure!(
        bytes <= RESULT_MAX_BYTES,
        "receipt result of {bytes} bytes exceeds {RESULT_MAX_BYTES}"
    );
    let now = corral_core::clock::now();
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
    Ok(receipt)
}

pub(crate) fn pm_selection(origin: &RequestOrigin) -> pm_state::Selection {
    let selection = pm_state::Selection::request(origin.binding.generation, &origin.request_id);
    if origin.op == "ask" {
        selection.family("questions")
    } else {
        selection
    }
}

/// Commits `apply` and its receipt in one transaction of the named container. `apply` sees the
/// container's closure reason, read in the same transaction.
pub(crate) fn commit(
    ctx: &Ctx,
    admitted: &Admitted,
    apply: impl FnOnce(&mut AgentRunState, Option<ClosureReason>) -> Result<Commit>,
) -> Result<CallResponse> {
    commit_selected(ctx, admitted, &pm_selection(&admitted.origin), apply)
}

/// PM callers name any additional child records their apply step consumes. Other roles
/// retain their existing transaction boundaries.
pub(crate) fn commit_selected(
    ctx: &Ctx,
    admitted: &Admitted,
    selection: &pm_state::Selection,
    apply: impl FnOnce(&mut AgentRunState, Option<ClosureReason>) -> Result<Commit>,
) -> Result<CallResponse> {
    failpoint("agent_admitted_before_commit");
    let origin = &admitted.origin;
    let container = &origin.binding.container;
    let mut outcome: Option<(RequestReceipt, bool)> = None;
    if container.assistant().is_some() {
        return commit_assistant(ctx, admitted, |assistant| apply(&mut assistant.state, None));
    }
    match container.task() {
        None => pm_state::update_selected(&ctx.store, selection, |pm| {
            let pm = pm.as_mut().ok_or_else(|| {
                Rejection::new(RejectCode::ContainerNotFound, "the PM binding disappeared")
            })?;
            if let Some(stored) = pm.state.receipt(&origin.request_id) {
                admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
                outcome = Some((stored.clone(), true));
                return Ok(((), false));
            }
            admission::recheck_pm(pm, origin.binding.generation)?;
            pm_capacity(&ctx.store)?;
            let commit = apply(&mut pm.state, None)?;
            outcome = Some((append(&mut pm.state, origin, commit)?, false));
            Ok(((), true))
        })?,
        Some(_) => {
            return commit_with_task(ctx, admitted, |task| {
                let closure = agent::container_closure(task, container);
                let state = agent::container_state_mut(task, container)
                    .context("the agent binding disappeared")?;
                apply(state, closure)
            });
        }
    }
    let (receipt, replayed) = outcome.context("the commit produced no receipt")?;
    respond(ctx, container, receipt, replayed)
}

/// A task mutation and its receipt share one head transaction; no accumulated native vectors
/// are loaded. PM operations keep their separate journal and do not enter this task-only seam.
pub(crate) fn commit_head(
    ctx: &Ctx,
    admitted: &Admitted,
    apply: impl FnOnce(&mut corral_core::TaskHeadMut) -> Result<Commit>,
) -> Result<CallResponse> {
    failpoint("agent_admitted_before_commit");
    let origin = &admitted.origin;
    let container = &origin.binding.container;
    let task_id = container
        .task()
        .context("a head commit names a task container")?;
    let mut outcome = None;
    ctx.store.try_update_head(task_id, |task| {
        if let Some(stored) = task.agent_receipt(container, &origin.request_id)? {
            admission::same_request(&stored, &origin.op, &origin.payload_sha256)?;
            outcome = Some((stored, true));
            return Ok(false);
        }
        admission::recheck_task_head(task, &origin.binding)?;
        let commit = apply(task)?;
        let head = task
            .agent_head(container)?
            .context("the agent binding disappeared")?;
        if head.records.receipt_count() >= RECEIPTS_MAX as u64 {
            return Err(Rejection::new(
                RejectCode::ReceiptLimit,
                format!("the container already holds {RECEIPTS_MAX} receipts"),
            )
            .into());
        }
        let receipt = new_receipt(origin, commit)?;
        task.put_agent_record(container, agent::NativeRecord::Receipt(receipt.clone()))?;
        outcome = Some((receipt, false));
        Ok(true)
    })?;
    let (receipt, replayed) = outcome.context("the head commit produced no receipt")?;
    respond(ctx, container, receipt, replayed)
}

/// How a commit transaction answers an identical receipt that is already stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoredAnswer {
    /// Answer it as-is, even to a fenced generation: every op's replay-after-fence rule.
    Always,
    /// Answer it only to a live, current generation of an open session, so a receipt that
    /// is authority to write (a write_permission decide) never reaches a fenced or ended host.
    LiveOnly,
}

/// `commit` for an assistant session whose op also changes its container beyond the run state,
/// such as the owner-input journal, in the same transaction as its receipt.
pub(crate) fn commit_assistant(
    ctx: &Ctx,
    admitted: &Admitted,
    apply: impl FnOnce(&mut AssistantContainer) -> Result<Commit>,
) -> Result<CallResponse> {
    commit_assistant_with(ctx, admitted, StoredAnswer::Always, apply)
}

/// `commit_assistant` whose existing-receipt branch answers as `answer` says.
pub(crate) fn commit_assistant_with(
    ctx: &Ctx,
    admitted: &Admitted,
    answer: StoredAnswer,
    apply: impl FnOnce(&mut AssistantContainer) -> Result<Commit>,
) -> Result<CallResponse> {
    let origin = &admitted.origin;
    let container = &origin.binding.container;
    let id = container
        .assistant()
        .context("an assistant commit names an assistant session")?;
    let (receipt, replayed) = ctx
        .store
        .settle_assistant_container(id, |assistant, ended| {
            if let Some(stored) = assistant.state.receipt(&origin.request_id) {
                admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
                if answer == StoredAnswer::LiveOnly {
                    admission::recheck_assistant(assistant, ended, origin.binding.generation)?;
                }
                return Ok((stored.clone(), true));
            }
            admission::recheck_assistant(assistant, ended, origin.binding.generation)?;
            let commit = apply(assistant)?;
            Ok((append(&mut assistant.state, origin, commit)?, false))
        })?;
    respond(ctx, container, receipt, replayed)
}

/// Answers a receipt that admission found only to a live generation: under the session's store
/// lock, the lock fences and ends take, it rereads the receipt and repeats `same_request` and
/// the liveness, fence and saved-pane check before `respond`.
pub(crate) fn replay_assistant_live(
    ctx: &Ctx,
    envelope: &Envelope,
    stored: RequestReceipt,
) -> Result<CallResponse> {
    let container = admission::container_of(&envelope.identity);
    let id = container
        .assistant()
        .context("a live replay names an assistant session")?;
    let receipt = ctx
        .store
        .inspect_assistant_container(id, |assistant, ended| {
            let current = assistant
                .state
                .receipt(&envelope.request_id)
                .cloned()
                .unwrap_or(stored);
            admission::same_request(&current, &envelope.op, &envelope.payload_sha256)?;
            admission::recheck_assistant(assistant, ended, envelope.identity.generation)?;
            Ok(current)
        })?;
    respond(ctx, &container, receipt, true)
}

/// `commit` for a worker run or review child whose op also changes task-level state (status,
/// blocked excerpt) in the same transaction as its receipt.
pub(crate) fn commit_with_task(
    ctx: &Ctx,
    admitted: &Admitted,
    apply: impl FnOnce(&mut corral_core::Task) -> Result<Commit>,
) -> Result<CallResponse> {
    commit_with_task_scope(ctx, admitted, apply, false)
}

/// T-626's retained-vector mutation only; bounded post-commit work stays unattributed.
pub(crate) fn commit_native_event(
    ctx: &Ctx,
    admitted: &Admitted,
    turn: u64,
    apply: impl FnOnce(&mut AgentRunState, Option<ClosureReason>) -> Result<Commit>,
) -> Result<CallResponse> {
    let container = &admitted.origin.binding.container;
    if container.task().is_none() {
        let selection = pm_selection(&admitted.origin).key(
            "turns",
            format!("turn:{}:{turn}", admitted.origin.binding.generation),
        );
        return commit_selected(ctx, admitted, &selection, apply);
    }
    failpoint("agent_admitted_before_commit");
    commit_with_task_scope(
        ctx,
        admitted,
        |task| {
            let closure = agent::container_closure(task, container);
            let state = agent::container_state_mut(task, container)
                .context("the agent binding disappeared")?;
            apply(state, closure)
        },
        true,
    )
}

fn commit_with_task_scope(
    ctx: &Ctx,
    admitted: &Admitted,
    apply: impl FnOnce(&mut corral_core::Task) -> Result<Commit>,
    retention: bool,
) -> Result<CallResponse> {
    let origin = &admitted.origin;
    let container = &origin.binding.container;
    let task_id = container
        .task()
        .context("a task commit names a worker run or review child")?;
    let mut outcome: Option<(RequestReceipt, bool)> = None;
    let known = retention.then(|| corral_core::work::followup("T-626", "native_event_retention"));
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
        let commit = apply(task)?;
        let state =
            agent::container_state_mut(task, container).context("the agent binding disappeared")?;
        outcome = Some((append(state, origin, commit)?, false));
        Ok(true)
    })?;
    drop(known);
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
    for key in ["kind", "turn", "class", "budget"] {
        if let Some(value) = receipt.result.get(key).filter(|value| !value.is_null()) {
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
    match (container.assistant(), container.task()) {
        (Some(id), _) => {
            store.settle_assistant_container(id, |assistant, _| Ok(clear(&mut assistant.state)))?;
        }
        (None, None) => pm_state::update_selected(
            store,
            &pm_state::Selection::default()
                .key("receipts", format!("request:{}", receipt.request_id)),
            |pm| {
                let changed = pm.as_mut().is_some_and(|pm| clear(&mut pm.state));
                Ok(((), changed))
            },
        )?,
        (None, Some(task_id)) => {
            store.try_update_head(task_id, |task| {
                let Some(mut stored) = task.agent_receipt(container, &receipt.request_id)? else {
                    return Ok(false);
                };
                let before = stored.post_pending.len();
                stored.post_pending.retain(|entry| !events.contains(entry));
                if stored.post_pending.len() == before {
                    return Ok(false);
                }
                task.put_agent_record(container, agent::NativeRecord::Receipt(stored))?;
                Ok(true)
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
