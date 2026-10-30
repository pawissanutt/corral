//! The single write path for portable waits, owner decisions and resume receipts.
use anyhow::{Context, Result, ensure};
use corral_core::{
    DecisionRecord, DecisionRequest, ReceiptOutcome, ResumeDelivery, ResumeReceipt, Run, Status,
    Task, WaitHandle, WaitObservation, WaitRecord, WaitRequest, WaitState,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::agent::effects::{self, Receipted};
use corral_core::agent::{RequestOrigin, RequestReceipt, WakeKind, WakeState, WakeSubscription};

fn identity(version: u32, id: &str) -> Result<()> {
    ensure!(version == 1, "unsupported obligation version {version}");
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)),
        "invalid obligation or receipt id"
    );
    Ok(())
}

fn live_run<'a>(task: &'a mut Task, run: &str) -> Result<&'a mut Run> {
    ensure!(
        task.status.holds_slot(),
        "task does not have an active worker"
    );
    task.current_run_mut()
        .filter(|r| r.id == run && r.outcome.is_none())
        .context("mismatched_run: receipt or registration is not for the current open run")
}

fn unused_id(run: &Run, id: &str) -> Result<()> {
    ensure!(
        !run.obligations.waits.iter().any(|w| w.request.id == id)
            && !run.obligations.decisions.iter().any(|d| d.request.id == id),
        "conflicting obligation id {id}"
    );
    Ok(())
}

pub fn register_wait(ctx: &Ctx, id: &str, request: WaitRequest, now: Timestamp) -> Result<Task> {
    wait_inner(ctx, id, request, now, None)?;
    ctx.store.get(id)
}

/// The Pi-native `wait` op: its receipt and wake subscription commit with the wait.
pub fn register_wait_request(
    ctx: &Ctx,
    id: &str,
    request: WaitRequest,
    now: Timestamp,
    origin: &RequestOrigin,
) -> Result<Receipted> {
    let home = effects::Home::of(origin);
    effects::swept(ctx, origin, home, || {
        wait_inner(ctx, id, request, now, Some(origin))
    })
}

fn wait_inner(
    ctx: &Ctx,
    id: &str,
    request: WaitRequest,
    now: Timestamp,
    origin: Option<&RequestOrigin>,
) -> Result<Option<RequestReceipt>> {
    identity(request.version, &request.id)?;
    ensure!(
        !request.condition.trim().is_empty(),
        "wait condition is empty"
    );
    ensure!(!request.owner.trim().is_empty(), "wait owner is empty");
    let detail = json!({"run":request.run,"wait":request.id,"condition":request.condition});
    let mut receipt = None;
    if origin.is_some() {
        effects::failpoint("swept_before_commit");
    }
    let written = ctx.store.try_update(id, |t| {
        if let Some(origin) = origin {
            effects::recheck(&ctx.store, t, origin, &effects::Home::of(origin))?;
        }
        let own = t.id.clone();
        let run = live_run(t, &request.run)?;
        let parked = run.parked_on(&own);
        if let Some(old) = run
            .obligations
            .waits
            .iter()
            .find(|w| w.request.id == request.id)
        {
            ensure!(old.request == request, "conflicting wait id {}", request.id);
            let Some(origin) = origin else {
                return Ok(false);
            };
            let commit = effects::Commit {
                result: json!({"wait": request.id, "registered": false}),
                effect: Some(("wait", request.id.clone())),
                owed: Vec::new(),
            };
            receipt = Some(effects::record(
                t,
                origin,
                &effects::Home::of(origin),
                commit,
            )?);
            return Ok(true);
        }
        unused_id(run, &request.id)?;
        if let WaitHandle::Gate { sha, pane } = &request.handle {
            ensure!(
                run.gate
                    .as_ref()
                    .is_some_and(|g| &g.sha == sha && g.pane.as_ref() == Some(pane)),
                "gate handle does not match the current corral gate"
            );
        }
        run.obligations.waits.push(WaitRecord {
            request: request.clone(),
            registered_at: now,
            observation: WaitObservation {
                at: now,
                state: WaitState::Pending,
                detail: "registered; reconciliation pending".into(),
            },
            receipt: None,
        });
        run.obligations.settle_parking(&parked, &own, now);
        if let Some(origin) = origin {
            let commit = effects::Commit {
                result: json!({
                    "wait": request.id, "registered": true,
                    "event": effects::event("wait_registered", detail.clone(), origin),
                }),
                effect: Some(("wait", request.id.clone())),
                owed: vec!["effect:event", "effect:reconcile_wait"],
            };
            receipt = Some(effects::record(
                t,
                origin,
                &effects::Home::of(origin),
                commit,
            )?);
            effects::subscribe_wait(t, origin, &request.id)?;
        }
        Ok(true)
    })?;
    if origin.is_none()
        && let Some(task) = written
    {
        ctx.store
            .log_event(Some(&task.id), "wait_registered", detail)?;
        // Classify capabilities/deadlines immediately; duplicate registration is a true no-op.
        super::obligations_sync::reconcile_one(ctx, id, now, None)?;
    }
    Ok(receipt)
}

pub fn register_decision(
    ctx: &Ctx,
    id: &str,
    request: DecisionRequest,
    now: Timestamp,
) -> Result<Task> {
    identity(request.version, &request.id)?;
    ensure!(
        !request.question.trim().is_empty(),
        "decision question is empty"
    );
    ensure!(
        !request.requested_action.trim().is_empty(),
        "requested action is empty"
    );
    ensure!(!request.owner.trim().is_empty(), "decision owner is empty");
    let written = ctx.store.try_update(id, |t| {
        let own = t.id.clone();
        let run = live_run(t, &request.run)?;
        let parked = run.parked_on(&own);
        if let Some(old) = run
            .obligations
            .decisions
            .iter()
            .find(|d| d.request.id == request.id)
        {
            ensure!(
                old.request == request,
                "conflicting decision id {}",
                request.id
            );
            return Ok(false);
        }
        unused_id(run, &request.id)?;
        run.obligations.decisions.push(DecisionRecord {
            request: request.clone(),
            registered_at: now,
            observation: WaitObservation {
                at: now,
                state: WaitState::Pending,
                detail: "awaiting owner answer".into(),
            },
            receipt: None,
        });
        run.obligations.settle_parking(&parked, &own, now);
        run.blocked_excerpt = Some(format!(
            "{}\nAction: {}",
            request.question, request.requested_action
        ));
        t.status = Status::NeedsMe;
        Ok(true)
    })?;
    if let Some(task) = written {
        transition(
            ctx,
            &task.id,
            "needs_me",
            json!({"run":request.run,"decision":request.id,"question":request.question,"requested_action":request.requested_action,"owner":request.owner}),
        )?;
    }
    ctx.store.get(id)
}

#[derive(Debug, Clone, Copy)]
pub enum ResumeTarget<'a> {
    Wait(&'a str),
    Decision(&'a str),
}

/// Persists an event receipt and wakes the run's worker: a newly recorded receipt is delivered to
/// its session as a prompt, so an idle or parked worker continues without anyone typing into its
/// pane. This never launches, replays or kills a process.
pub fn resume_run(
    ctx: &Ctx,
    id: &str,
    target: ResumeTarget<'_>,
    receipt: ResumeReceipt,
    now: Timestamp,
) -> Result<Task> {
    let (task, recorded) = record_receipt(ctx, id, target, &receipt, now)?;
    if recorded {
        wake_worker(ctx, &task, target, &receipt);
    }
    Ok(task)
}

/// Prompts the worker of a still-running run; one queued for a coding slot is told to wait for it.
/// A failed delivery keeps the receipt and is reported.
fn wake_worker(ctx: &Ctx, task: &Task, target: ResumeTarget<'_>, receipt: &ResumeReceipt) {
    let Some(run) = task
        .current_run()
        .filter(|r| task.status == Status::Running && r.id == receipt.run && r.outcome.is_none())
    else {
        return;
    };
    // A native worker's armed wake for this wait is posted by sync; never prompt it twice.
    let native = |wake: &WakeSubscription| {
        wake.kind == WakeKind::Wait
            && wake.state == WakeState::Armed
            && matches!(target, ResumeTarget::Wait(wait) if wake.target == wait)
    };
    if run
        .agent_host
        .as_deref()
        .is_some_and(|s| s.wakes.iter().any(native))
    {
        return;
    }
    let event = match target {
        ResumeTarget::Wait(wait) => format!("wait `{wait}` resolved: {:?}", receipt.outcome),
        ResumeTarget::Decision(decision) => format!(
            "decision `{decision}` answered: {}",
            receipt.answer.as_deref().unwrap_or_default()
        ),
    };
    let next = match &run.obligations.slot_wait {
        Some(slot) => format!(
            "Do not resume work until `corral wait --for slot_reacquired --task {} --since {}` returns, then continue.",
            task.id, slot.since
        ),
        None => "Continue the run.".into(),
    };
    let message = format!(
        "corral: {} {} {event} (receipt {}). {next}",
        task.id, run.id, receipt.id
    );
    if let Err(error) = crate::reply(ctx, &task.id, &message) {
        crate::messages::report(
            Some(&task.id),
            "resume",
            format!("receipt recorded; waking the worker failed: {error:#}"),
        );
    }
}

fn record_receipt(
    ctx: &Ctx,
    id: &str,
    target: ResumeTarget<'_>,
    receipt: &ResumeReceipt,
    now: Timestamp,
) -> Result<(Task, bool)> {
    identity(receipt.version, &receipt.id)?;
    let written = ctx.store.try_update(id, |t| {
        let own = t.id.clone();
        let finished_decision = matches!(target, ResumeTarget::Decision(_))
            && t.current_run().is_some_and(|r| r.id == receipt.run && r.outcome.is_some());
        let run = if finished_decision {
            t.current_run_mut().context("run disappeared")?
        } else {
            live_run(t, &receipt.run)?
        };
        let parked = run.parked_on(&own);
        let existing = run.obligations.waits.iter().filter_map(|w| w.receipt.as_ref())
            .chain(run.obligations.decisions.iter().filter_map(|d| d.receipt.as_ref()))
            .find(|r| r.id == receipt.id).cloned();
        if let Some(existing) = &existing {
            ensure!(existing == receipt, "conflicting receipt id {}", receipt.id);
        }
        let (registered, slot, observation) = match target {
            ResumeTarget::Wait(wait) => {
                let w = run.obligations.waits.iter_mut().find(|w| w.request.id == wait).context("unknown wait id")?;
                ensure!(receipt.outcome != ReceiptOutcome::Answered, "a wait cannot be answered");
                ensure!(!matches!(w.request.handle, WaitHandle::Gate { .. }), "corral gate receipts are collected from the actual gate result, not worker assertions");
                (w.registered_at, &mut w.receipt, &mut w.observation)
            }
            ResumeTarget::Decision(decision) => {
                let d = run.obligations.decisions.iter_mut().find(|d| d.request.id == decision).context("unknown decision id")?;
                ensure!(receipt.outcome == ReceiptOutcome::Answered, "a decision requires an answered receipt");
                ensure!(receipt.answer.as_deref().is_some_and(|a| !a.trim().is_empty()), "decision answer is empty");
                (d.registered_at, &mut d.receipt, &mut d.observation)
            }
        };
        if let Some(old) = slot {
            ensure!(old == receipt, "conflicting terminal receipt");
            return Ok(false);
        }
        ensure!(existing.is_none(), "receipt id is already used by another obligation");
        ensure!(receipt.observed_at >= registered && receipt.observed_at <= now, "receipt observation is outside this registration's lifetime");
        *observation = receipt_observation(receipt, now);
        *slot = Some(receipt.clone());
        run.obligations.settle_parking(&parked, &own, now);
        if !finished_decision {
            if let Some(pending) = run.obligations.pending_decision() {
                run.blocked_excerpt = Some(format!("{}\nAction: {}", pending.request.question, pending.request.requested_action));
                t.status = Status::NeedsMe;
            } else if t.status == Status::NeedsMe {
                t.status = Status::Running;
                let run = t.current_run_mut().context("run disappeared")?;
                run.blocked_excerpt = None;
            }
        } else if run.obligations.pending_decision().is_none() {
            run.blocked_excerpt = None;
        }
        Ok(true)
    })?;
    let recorded = written.is_some();
    if let Some(task) = written {
        transition(
            ctx,
            &task.id,
            "run_resumed",
            json!({"run":receipt.run,"receipt":receipt.id,"outcome":receipt.outcome,"delivery":receipt.delivery,
                "decision":match target { ResumeTarget::Decision(decision) => Some(decision), ResumeTarget::Wait(_) => None }}),
        )?;
        if task.current_run().is_some_and(|run| run.outcome.is_none()) {
            crate::parked::reacquire(ctx, &task.id)?;
        }
    }
    Ok((ctx.store.get(id)?, recorded))
}

pub(crate) fn answer_reply(
    ctx: &Ctx,
    id: &str,
    run: &str,
    decision: &str,
    answer: &str,
) -> Result<Task> {
    let now = Timestamp::now();
    answer_reply_with(
        ctx,
        id,
        &ResumeReceipt {
            version: 1,
            id: decision.to_string(),
            run: run.to_string(),
            outcome: ReceiptOutcome::Answered,
            delivery: ResumeDelivery::Acknowledged,
            observed_at: now,
            answer: Some(answer.to_string()),
            data: json!({"via":"corral_reply"}),
        },
    )
}

/// Commits an exact, previously journaled answer. Replaying the identical receipt is a no-op,
/// so an interrupted inbox effect can finish without generating a second answer. The reply that
/// carries the answer is the worker's prompt, so this records without a separate wake.
pub(crate) fn answer_reply_with(ctx: &Ctx, id: &str, receipt: &ResumeReceipt) -> Result<Task> {
    record_receipt(
        ctx,
        id,
        ResumeTarget::Decision(&receipt.id),
        receipt,
        Timestamp::now(),
    )
    .map(|(task, _)| task)
}

pub(crate) fn receipt_observation(receipt: &ResumeReceipt, now: Timestamp) -> WaitObservation {
    let state = match receipt.delivery {
        ResumeDelivery::Unknown => WaitState::DeliveryUnknown,
        ResumeDelivery::Acknowledged => match receipt.outcome {
            ReceiptOutcome::Ready | ReceiptOutcome::Answered => WaitState::Ready,
            ReceiptOutcome::Failed => WaitState::Failed,
            ReceiptOutcome::Cancelled => WaitState::Cancelled,
            ReceiptOutcome::Lost => WaitState::Lost,
        },
    };
    WaitObservation {
        at: now,
        state,
        detail: format!(
            "receipt {}: {:?}; delivery {:?}",
            receipt.id, receipt.outcome, receipt.delivery
        ),
    }
}

pub(crate) fn transition(ctx: &Ctx, id: &str, kind: &str, detail: serde_json::Value) -> Result<()> {
    transition_with_notice(ctx, id, kind, detail, None)
}

pub(crate) fn transition_with_notice(
    ctx: &Ctx,
    id: &str,
    kind: &str,
    detail: serde_json::Value,
    pending: Option<crate::owner_notice::PendingNotice>,
) -> Result<()> {
    ctx.store.log_event(Some(id), kind, detail.clone())?;
    let committed =
        if let Some(pending) = pending {
            Some(crate::owner_notice::newly_committed(
                &ctx.store.get(id)?,
                pending,
            )?)
        } else {
            let source = crate::verification::digest(&serde_json::to_vec(&(kind, &detail))?);
            crate::owner_notice::commit(ctx, id, |_, task| {
                if task.attention.notice_outbox.iter().any(|claim| {
                    claim.producer == "obligation_transition" && claim.source == source
                }) {
                    return Ok(None);
                }
                Ok(Some(crate::owner_notice::prepare(
                    ctx,
                    task,
                    "obligation_transition",
                    &source,
                    Timestamp::now(),
                    Vec::new(),
                )?))
            })?
        };
    if let Some(committed) = committed {
        crate::owner_notice::notify(
            ctx,
            committed,
            &format!("{id} {kind}"),
            &detail.to_string(),
            true,
        )?;
    }
    Ok(())
}
