//! Gate receipts are the admission queue. Sync owns retries, never the model.
use anyhow::{Context, Result};
use corral_core::agent::{
    self, ContainerRef, Executor, ReceiptState, RequestBinding, RequestOrigin, RequestReceipt,
};
use serde_json::json;

use super::{admission, effects};
use crate::Ctx;

fn transient(error: &anyhow::Error) -> bool {
    error
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
    ctx.store.try_update(id, |task| {
        let state = agent::container_state_mut(task, &origin.binding.container)
            .context("gate binding disappeared")?;
        let receipt = state
            .receipts
            .iter_mut()
            .find(|r| r.request_id == origin.request_id)
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
        task.review_wait = Some(corral_core::ReviewWork::Gate);
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
        attempt: effects::own_attempt(&origin.request_id),
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
    let task = ctx.store.get(task_id)?;
    let current = agent::container_state(&task, &origin.binding.container)
        .and_then(|s| s.receipt(&origin.request_id))
        .cloned()
        .context("gate reservation disappeared")?;
    let settled = crate::review_budget::settle(ctx, &current, Some(&me), true, error)?;
    effects::failpoint("gate_receipt_before_ack");
    super::receipts::respond(ctx, &origin.binding.container, settled, replayed)
}

/// Called after sync drops its shared pane-inventory lease, before admitting new work.
pub(crate) fn advance(ctx: &Ctx) -> Result<()> {
    let mut queued = Vec::new();
    for task in ctx.store.list()? {
        for run in &task.runs {
            if let Some(state) = run.agent_host.as_deref() {
                queued.extend(
                    state
                        .receipts
                        .iter()
                        .filter(|r| r.op == "gate" && r.state == ReceiptState::Queued)
                        .cloned(),
                );
            }
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
        ctx.store.try_update(id, |task| {
            if agent::check_live(task, &origin.binding).is_err() {
                return Ok(false);
            }
            admission::recheck_task(task, &origin.binding)?;
            let state = agent::container_state_mut(task, &origin.binding.container)
                .context("queued gate binding disappeared")?;
            let Some(stored) = state.receipts.iter_mut().find(|r| {
                r.request_id == receipt.request_id
                    && r.state == ReceiptState::Queued
                    && r.executor == receipt.executor
            }) else {
                return Ok(false);
            };
            stored.executor = Some(me.clone());
            claimed = true;
            Ok(true)
        })?;
        if !claimed {
            // A fenced/closed request is cancelled by the same evidence-based reconciler.
            effects::reconcile_task(ctx, id)?;
            continue;
        }
        effects::failpoint("gate_queue_claimed");
        execute(ctx, &origin, me.clone(), true)?;
        let task = ctx.store.get(id)?;
        let still_queued = agent::container_state(&task, &origin.binding.container)
            .and_then(|s| s.receipt(&origin.request_id))
            .is_some_and(|r| r.state == ReceiptState::Queued);
        let due = effects::due_gate_wakes(
            &task,
            jiff::Timestamp::now(),
            std::time::Duration::from_secs(ctx.store.config.agent.gate_start_uncertain_secs),
        );
        super::wake::post_due(ctx, &due)?;
        if still_queued {
            break;
        }
    }
    Ok(())
}
