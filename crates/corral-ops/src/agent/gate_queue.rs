//! Gate receipts are the admission queue. Sync owns retries, never the model.
use anyhow::{Context, Result};
use corral_core::TaskRef;
use corral_core::agent::{
    self, ContainerRef, Executor, ReceiptState, RequestBinding, RequestOrigin, RequestReceipt,
};
use serde_json::json;

use super::{admission, effects};
use crate::Ctx;

fn transient(error: &anyhow::Error) -> bool {
    error.is::<crate::hold::Held>()
        || crate::operating::transient_refusal(error)
        || error
            .downcast_ref::<crate::review_budget::CapacityWait>()
            .is_some()
        || error
            .downcast_ref::<crate::review_budget::LaunchBusy>()
            .is_some()
}

fn enqueue(
    ctx: &Ctx,
    origin: &RequestOrigin,
    executor: &Executor,
    reason: &str,
) -> Result<RequestReceipt> {
    let id = origin.binding.container.task().context("gate task")?;
    let mut out = None;
    ctx.store.try_update_head(id, |task| {
        let mut receipt = task
            .agent_receipt(&origin.binding.container, &origin.request_id)?
            .context("gate receipt disappeared")?;
        if !matches!(receipt.state, ReceiptState::Reserved | ReceiptState::Queued)
            || receipt.executor.as_ref() != Some(executor)
        {
            out = Some(receipt.clone());
            return Ok(false);
        }
        receipt.state = ReceiptState::Queued;
        receipt.executor = None;
        receipt.result = json!({
            "outcome": "queued",
            "attempt": receipt.effect.as_ref().map(|e| &e.reference),
            "reason": crate::review_budget::bounded(reason),
        });
        out = Some(receipt.clone());
        task.put_agent_record(
            &origin.binding.container,
            agent::NativeRecord::Receipt(receipt),
        )?;
        task.review_wait = crate::hold::require_clear(&ctx.store, &*task)
            .is_ok()
            .then_some(corral_core::ReviewWork::Gate);
        Ok(true)
    })?;
    out.context("queue update produced no receipt")
}

fn origin(receipt: &RequestReceipt) -> RequestOrigin {
    RequestOrigin {
        request_id: receipt.request_id.clone(),
        op: receipt.op.clone(),
        payload_sha256: receipt.payload_sha256.clone(),
        identity: receipt.identity.clone(),
        binding: RequestBinding {
            container: ContainerRef::Worker {
                task: receipt.identity.task.clone().unwrap_or_default(),
                run: receipt.identity.run.clone().unwrap_or_default(),
            },
            generation: receipt.identity.generation,
            callback: receipt.identity.callback.clone(),
            settles_turn: None,
        },
    }
}

pub(super) fn execute(
    ctx: &Ctx,
    origin: &RequestOrigin,
    me: Executor,
    replayed: bool,
) -> Result<super::CallResponse> {
    let task_id = origin.binding.container.task().context("gate task")?;
    let request = crate::review_budget::GateRequest {
        request_id: origin.request_id.clone(),
        attempt: ctx
            .store
            .head(task_id)?
            .agent_receipt(&origin.binding.container, &origin.request_id)?
            .and_then(|r| r.effect.map(|e| e.reference))
            .unwrap_or_else(|| effects::own_attempt(&origin.request_id)),
        binding: origin.binding.clone(),
    };
    let outcome = crate::gate::gate_start_request(ctx, task_id, request);
    let error = match &outcome {
        Ok(_) => {
            effects::failpoint("gate_started_before_receipt");
            None
        }
        Err(error) => {
            effects::failpoint("gate_refused_before_receipt");
            if transient(error) {
                let queued = enqueue(ctx, origin, &me, &format!("{error:#}"))?;
                return super::receipts::respond(ctx, &origin.binding.container, queued, replayed);
            }
            Some(format!("{error:#}"))
        }
    };
    let current = ctx
        .store
        .head(task_id)?
        .agent_receipt(&origin.binding.container, &origin.request_id)?
        .context("gate reservation disappeared")?;
    let settled = crate::review_budget::settle(ctx, &current, Some(&me), true, error)?;
    effects::failpoint("gate_receipt_before_ack");
    super::receipts::respond(ctx, &origin.binding.container, settled, replayed)
}

/// Preserve the worker's outstanding gate request and wake while replacing a cancelled
/// attempt with a new immutable attempt. No model turn or new request is needed.
pub(crate) fn requeue_cancelled(ctx: &Ctx, task: &mut corral_core::TaskHeadMut) -> Result<()> {
    let Some(run) = task.head().current_run.as_deref() else {
        return Ok(());
    };
    let Some(old) = run
        .gate
        .as_ref()
        .filter(|g| g.result == corral_core::GateResult::Cancelled)
        .and_then(|g| g.identity.as_ref())
        .map(|i| i.attempt.clone())
    else {
        return Ok(());
    };
    for (container, mut wake) in task.active_worker_wakes()? {
        if wake.kind != agent::WakeKind::Gate || wake.target != old {
            continue;
        }
        let Some(mut receipt) = task.agent_receipt(&container, &wake.request_id)? else {
            continue;
        };
        let next = format!(
            "{}-c{}",
            effects::own_attempt(&receipt.request_id),
            ctx.store.now().as_nanosecond()
        );
        if let Some(effect) = receipt.effect.as_mut() {
            effect.reference = next.clone();
        }
        receipt.state = ReceiptState::Queued;
        receipt.executor = None;
        receipt.result = json!({"outcome":"queued", "attempt":next, "requeued_from":old, "reason":"previous attempt cancelled"});
        wake.target = next;
        wake.state = agent::WakeState::Armed;
        task.put_agent_record(&container, agent::NativeRecord::Receipt(receipt))?;
        task.put_agent_record(&container, agent::NativeRecord::Wake(wake))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn advance(ctx: &Ctx) -> Result<()> {
    advance_heads(ctx, &ctx.store.unfinished_native_heads()?)
}

/// Called after sync drops its shared pane-inventory lease, before admitting new work, with the
/// board the pass already read: every claim below rechecks its task under the store lock, and a
/// gate queued after that read is admitted on the next pass.
pub(crate) fn advance_heads(ctx: &Ctx, tasks: &[corral_core::TaskHead]) -> Result<()> {
    let _span = corral_core::timing::span("sync.advance_heads");
    let mut queued = Vec::new();
    for task in tasks {
        if task.hold.is_none() {
            queued.extend(task.queued_agent_gates()?);
        }
    }
    queued.sort_by(|a, b| {
        a.at.cmp(&b.at)
            .then_with(|| a.request_id.cmp(&b.request_id))
    });
    if queued.is_empty() {
        return Ok(());
    }
    let me = effects::executor()?;
    for receipt in queued {
        if receipt.executor.as_ref().is_some_and(|e| e == &me) {
            continue;
        }
        if let Some(executor) = &receipt.executor
            && !effects::executor_gone(executor)?
        {
            continue;
        }
        let origin = origin(&receipt);
        let id = origin
            .binding
            .container
            .task()
            .context("queued gate task")?;
        let mut claimed = false;
        ctx.store.try_update_head(id, |task| {
            if crate::hold::require_clear(&ctx.store, &*task).is_err()
                || agent::check_live_ref(task, &origin.binding)?.is_err()
            {
                return Ok(false);
            }
            admission::recheck_task_head(task, &origin.binding)?;
            let Some(mut stored) = task
                .agent_receipt(&origin.binding.container, &receipt.request_id)?
                .filter(|r| r.state == ReceiptState::Queued && r.executor == receipt.executor)
            else {
                return Ok(false);
            };
            stored.executor = Some(me.clone());
            task.put_agent_record(
                &origin.binding.container,
                agent::NativeRecord::Receipt(stored),
            )?;
            claimed = true;
            Ok(true)
        })?;
        if !claimed {
            if crate::hold::require_clear(&ctx.store, &ctx.store.head(id)?).is_err() {
                continue;
            }
            // A fenced/closed request is cancelled by the same evidence-based reconciler.
            crate::review_budget::settle(ctx, &receipt, receipt.executor.as_ref(), false, None)?;
            continue;
        }
        effects::failpoint("gate_queue_claimed");
        execute(ctx, &origin, me.clone(), true)?;
        let task = ctx.store.head(id)?;
        let still_queued = task
            .agent_receipt(&origin.binding.container, &origin.request_id)?
            .is_some_and(|receipt| receipt.state == ReceiptState::Queued);
        let due = crate::review_budget::due_gate_wakes_ref(
            &task,
            ctx.store.now(),
            std::time::Duration::from_secs(ctx.store.config.agent.gate_start_uncertain_secs),
        )?;
        super::wake::post_due(ctx, &due)?;
        if still_queued {
            break;
        }
    }
    Ok(())
}
