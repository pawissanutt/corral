//! Restart-safe reconciliation of declared handles and actual corral gate results.
use std::collections::HashMap;

use anyhow::{Context, Result};
use corral_core::{
    GateResult, ReceiptOutcome, ResumeDelivery, ResumeReceipt, Run, TaskHead, WaitHandle,
    WaitObservation, WaitRecord, WaitState,
};
use corral_herdr::PaneInfo;
use jiff::Timestamp;
use serde_json::json;

use crate::obligations::{receipt_observation, transition};
use crate::{Change, Ctx};

pub(crate) fn reconcile_one(
    ctx: &Ctx,
    id: &str,
    now: Timestamp,
    panes: Option<&HashMap<String, PaneInfo>>,
) -> Result<Option<Change>> {
    reconcile_head(ctx, &ctx.store.head(id)?, now, panes)
}

/// Re-observes only the current run's open obligations: a wait or decision with a resume
/// receipt keeps the observation its receipt fixed, so no tick can change it.
pub(crate) fn reconcile_head(
    ctx: &Ctx,
    head: &TaskHead,
    now: Timestamp,
    panes: Option<&HashMap<String, PaneInfo>>,
) -> Result<Option<Change>> {
    if panes.is_some() && !crate::session_scope::owns_head(ctx, head)? {
        return Ok(None);
    }
    let Some(current) = head.current_run.as_ref().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    if !head.status.holds_slot() || current.facts.waits + current.facts.decisions == 0 {
        return Ok(None);
    }
    let shell = head.to_shell_task()?;
    let Some(run) = shell.current_run() else {
        return Ok(None);
    };
    let open = head.open_obligations(&run.id)?;
    let mut next = open.clone();
    let mut transitions = Vec::new();
    let mut task_wakes = Vec::new();
    for wait in &mut next.waits {
        let (state, detail, receipt) = if wait.receipt.is_none()
            && wait.observation.state == WaitState::Pending
            && wait.request.run == run.id
            && task_status_ready(ctx, wait)?
        {
            let receipt = ResumeReceipt {
                version: 1,
                id: format!("task-status:{}", wait.request.id),
                run: run.id.clone(),
                outcome: ReceiptOutcome::Ready,
                delivery: ResumeDelivery::Acknowledged,
                observed_at: now,
                answer: None,
                data: json!({"handle": wait.request.handle}),
            };
            let observation = receipt_observation(&receipt, now);
            (observation.state, observation.detail, Some(receipt))
        } else {
            observe(ctx, &head.id, run, wait, now, panes)
        };
        if let Some(receipt) = receipt {
            if matches!(wait.request.handle, WaitHandle::TaskStatus { .. }) {
                task_wakes.push((wait.request.id.clone(), receipt.clone()));
            }
            wait.receipt = Some(receipt);
        }
        if wait.observation.state != state || wait.observation.detail != detail {
            let changed_state = wait.observation.state != state;
            wait.observation = WaitObservation {
                at: now,
                state,
                detail,
            };
            if changed_state {
                transitions.push(
                json!({"wait":wait.request.id,"state":state,"condition":wait.request.condition}),
                );
            }
        }
    }
    for decision in &mut next.decisions {
        if decision.pending()
            && decision.request.deadline.is_some_and(|at| now >= at)
            && decision.observation.state != WaitState::Expired
        {
            decision.observation = WaitObservation {
                at: now,
                state: WaitState::Expired,
                detail: "owner decision deadline expired; answer still required".into(),
            };
            transitions.push(json!({"decision":decision.request.id,"state":"expired","question":decision.request.question}));
        }
    }
    next.settle_parking(&open.parked_on(&head.id), &head.id, now);
    if next == open {
        post_inbox_wakes(ctx, &head.id)?;
        return Ok(None);
    }
    let (status, rev) = (head.status, current.facts.obligations_rev);
    let written = ctx.store.try_update_head(&head.id, |t| {
        if panes.is_some() && !crate::session_scope::owns_head(ctx, t.head())? {
            return Ok(false);
        }
        if t.status != status {
            return Ok(false);
        }
        let Some(fresh) = t
            .current_run_mut()
            .filter(|r| r.id == run.id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        // obligations_rev stands in for the whole obligations: stricter only on A->B->A.
        if fresh.facts.obligations_rev != rev
            || fresh.obligations.slot_wait != open.slot_wait
            || fresh.gate != run.gate
        {
            return Ok(false);
        }
        fresh.obligations.slot_wait = next.slot_wait.clone();
        for (before, after) in open.waits.iter().zip(&next.waits) {
            if before != after {
                t.edit_wait(&run.id, &after.request.id, |w| {
                    *w = after.clone();
                    Ok(())
                })?;
            }
        }
        for (before, after) in open.decisions.iter().zip(&next.decisions) {
            if before != after {
                t.edit_decision(&run.id, &after.request.id, |d| {
                    *d = after.clone();
                    Ok(())
                })?;
            }
        }
        Ok(true)
    })?;
    let Some(task) = written else {
        return Ok(None);
    };
    for detail in transitions {
        transition(ctx, &task.id, "wait_transition", detail)?;
    }
    crate::parked::reacquire(ctx, &task.id)?;
    if !task_wakes.is_empty() {
        let current = ctx.store.get(&task.id)?;
        if current.current_run().is_some_and(|run| {
            run.delivery == corral_core::inbox::DeliveryMode::Legacy && run.agent_host.is_none()
        }) {
            for (wait, receipt) in &task_wakes {
                crate::obligations::wake_worker(
                    ctx,
                    &current,
                    crate::obligations::ResumeTarget::Wait(wait),
                    receipt,
                );
            }
        }
    }
    post_inbox_wakes(ctx, &task.id)?;
    Ok(Some(Change {
        task: task.id.clone(),
        from: task.status,
        to: task.status,
        note: next.lines().join("; "),
        stage: None,
    }))
}

fn post_inbox_wakes(ctx: &Ctx, id: &str) -> Result<()> {
    use corral_core::inbox::{
        DeliveryMode, Inbox, MAX_PAYLOAD, MessageBinding, Post, Recipient, Urgency,
    };
    let head = ctx.store.head(id)?;
    if !head.current_run.as_ref().is_some_and(|run| {
        run.outcome.is_none() && run.agent_host.is_none() && run.delivery == DeliveryMode::Inbox
    }) {
        return Ok(());
    }
    let task = ctx.store.get(id)?;
    let Some(run) = task.current_run().filter(|r| {
        r.outcome.is_none()
            && r.agent_host.is_none()
            && r.delivery == DeliveryMode::Inbox
            && !r.releases_slot(id)
    }) else {
        return Ok(());
    };
    let run_id = run.id.clone();
    let waits: Vec<_> = run
        .obligations
        .waits
        .iter()
        .filter(|w| {
            matches!(w.request.handle, WaitHandle::TaskStatus { .. })
                && w.receipt
                    .as_ref()
                    .is_some_and(|r| r.id == format!("task-status:{}", w.request.id))
                && w.wake_message.is_none()
        })
        .map(|w| w.request.id.clone())
        .collect();
    if waits.is_empty() {
        return Ok(());
    }
    let inbox = Inbox::new(
        &ctx.store,
        Recipient::Run {
            task: id.into(),
            run: run_id.clone(),
        },
    )?;
    for wait in waits {
        let _lock = inbox.lock()?;
        let mut message = None;
        ctx.store.try_update(id, |task| {
            let Some(run) = task.current_run().filter(|r| {
                r.id == run_id
                    && r.outcome.is_none()
                    && r.agent_host.is_none()
                    && r.delivery == DeliveryMode::Inbox
                    && !r.releases_slot(id)
            }) else {
                return Ok(false);
            };
            let Some(record) = run.obligations.waits.iter().find(|w| {
                w.request.id == wait
                    && w.wake_message.is_none()
                    && w.receipt
                        .as_ref()
                        .is_some_and(|r| r.id == format!("task-status:{wait}"))
            }) else {
                return Ok(false);
            };
            let body = format!(
                "Wait {wait} ({}) reached ready: {}",
                record.request.condition, record.observation.detail
            );
            let key = format!("wake:{id}/{run_id}/{wait}/ready");
            let posted = inbox.reserve_in(
                task,
                Post {
                    kind: "wait_wake",
                    urgency: Urgency::Wake,
                    author: "corral",
                    body: &body,
                    binding: MessageBinding {
                        run: Some(run_id.clone()),
                        ..MessageBinding::default()
                    },
                    subject: Some(json!({"key":key,"target":wait,"state":"ready"})),
                    max_payload: MAX_PAYLOAD,
                },
            )?;
            let run = task
                .current_run_mut()
                .context("run disappeared while posting wait wake")?;
            let record = run
                .obligations
                .waits
                .iter_mut()
                .find(|w| w.request.id == wait)
                .context("wait disappeared while posting wake")?;
            record.wake_message = Some(posted.id.clone());
            message = Some(posted);
            Ok(true)
        })?;
        if let Some(message) = message {
            inbox.announce(&message)?;
        }
    }
    Ok(())
}

fn task_status_ready(ctx: &Ctx, wait: &WaitRecord) -> Result<bool> {
    use corral_core::{Status, TaskWaitStatus};
    let WaitHandle::TaskStatus { task, status } = &wait.request.handle else {
        return Ok(false);
    };
    let target = match status {
        TaskWaitStatus::Done => Status::Done,
        TaskWaitStatus::Dropped => Status::Dropped,
    };
    if ctx.store.head(task)?.status == target {
        return Ok(true);
    }
    Ok(ctx.store.events_shared()?.iter().any(|event| {
        if event.task.as_deref() != Some(task.as_str()) || event.ts <= wait.registered_at {
            return false;
        }
        match target {
            Status::Done => {
                matches!(
                    event.kind.as_str(),
                    "approved" | "plan_accepted" | "tracker_completed"
                ) || (event.detail["to"] == "done" && event.detail["from"] != "done")
            }
            Status::Dropped => {
                event.kind == "discarded"
                    || (event.kind == "idea_decided"
                        && matches!(
                            event.detail["action"].as_str(),
                            Some("drop" | "decline" | "merge")
                        ))
                    || (event.detail["to"] == "dropped" && event.detail["from"] != "dropped")
            }
            _ => false,
        }
    }))
}

fn observe(
    ctx: &Ctx,
    task: &str,
    run: &Run,
    wait: &WaitRecord,
    now: Timestamp,
    panes: Option<&HashMap<String, PaneInfo>>,
) -> (WaitState, String, Option<ResumeReceipt>) {
    let state = |s, detail: &str| (s, detail.to_string(), None);
    if wait.request.run != run.id {
        return state(WaitState::MismatchedRun, "wait belongs to another run");
    }
    if let Some(receipt) = &wait.receipt {
        let observation = receipt_observation(receipt, now);
        return (observation.state, observation.detail, None);
    }
    if wait.request.deadline.is_some_and(|at| now >= at) {
        return state(WaitState::Expired, "declared wait deadline expired");
    }
    match &wait.request.handle {
        WaitHandle::TaskStatus { .. } => state(WaitState::Pending, "awaiting task status event"),
        WaitHandle::Channel {
            channel_id,
            capability,
        } => {
            if capability.as_deref() != Some("wait_v1") {
                state(
                    WaitState::Unsupported,
                    "adapter has not declared wait_v1 capability",
                )
            } else if channel_id.trim().is_empty() || wait.request.deadline.is_none() {
                state(
                    WaitState::Unverifiable,
                    "channel requires a stable handle and bounded deadline",
                )
            } else {
                state(
                    WaitState::Pending,
                    "declared channel; awaiting explicit receipt, wake delivery not guaranteed",
                )
            }
        }
        WaitHandle::Gate { sha, pane } => {
            let Some(gate) = run.gate.as_ref() else {
                return state(WaitState::Lost, "corral gate record is missing");
            };
            if &gate.sha != sha || gate.pane.as_ref() != Some(pane) {
                return state(
                    WaitState::MismatchedRun,
                    "gate invocation changed; old wait is not reusable",
                );
            }
            match gate.result {
                GateResult::Pass | GateResult::Fail | GateResult::Stale => {
                    // gate_collect is the only authority that turns the real result file into Pass/Fail.
                    let receipt = ResumeReceipt {
                        version: 1,
                        id: format!("corral-gate:{}", wait.request.id),
                        run: run.id.clone(),
                        outcome: if gate.result == GateResult::Pass {
                            ReceiptOutcome::Ready
                        } else {
                            ReceiptOutcome::Failed
                        },
                        delivery: ResumeDelivery::Unknown,
                        observed_at: now,
                        answer: None,
                        data: json!({"result_file":crate::gate_paths(ctx,task,&run.id).result,"gate":gate}),
                    };
                    let observation = receipt_observation(&receipt, now);
                    (observation.state, observation.detail, Some(receipt))
                }
                GateResult::Running => {
                    if crate::gate_paths(ctx, task, &run.id).result.exists() {
                        state(
                            WaitState::Unverifiable,
                            "gate result could not be collected",
                        )
                    } else {
                        match panes {
                            Some(panes) if !panes.contains_key(pane) => {
                                state(WaitState::Lost, "gate pane disappeared without a result")
                            }
                            Some(_) => {
                                state(WaitState::Pending, "awaiting the corral gate result file")
                            }
                            None => state(
                                WaitState::Unverifiable,
                                "gate pane availability has not been observed",
                            ),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "obligations_sync_guard_tests.rs"]
mod guard_tests;

#[cfg(test)]
#[path = "obligations_sync_differential_tests.rs"]
mod differential_tests;
