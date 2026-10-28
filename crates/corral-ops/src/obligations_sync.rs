//! Restart-safe reconciliation of declared handles and actual corral gate results.
use std::collections::HashMap;

use anyhow::Result;
use corral_core::{
    GateResult, ReceiptOutcome, ResumeDelivery, ResumeReceipt, Run, WaitHandle, WaitObservation,
    WaitRecord, WaitState,
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
    let task = ctx.store.get(id)?;
    if panes.is_some() && !crate::session_scope::owns_task(ctx, &task)? {
        return Ok(None);
    }
    let Some(run) = task.current_run().filter(|r| r.outcome.is_none()) else {
        return Ok(None);
    };
    if !task.status.holds_slot() || run.obligations.is_empty() {
        return Ok(None);
    }
    let mut next = run.obligations.clone();
    let mut transitions = Vec::new();
    for wait in &mut next.waits {
        let (state, detail, receipt) = observe(ctx, &task.id, run, wait, now, panes);
        if let Some(receipt) = receipt {
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
    next.settle_parking(&run.parked_on(&task.id), &task.id, now);
    if next == run.obligations {
        return Ok(None);
    }
    let written = ctx.store.try_update(id, |t| {
        if panes.is_some() && !crate::session_scope::owns_task(ctx, t)? {
            return Ok(false);
        }
        if t.status != task.status {
            return Ok(false);
        }
        let Some(current) = t
            .current_run_mut()
            .filter(|r| r.id == run.id && r.outcome.is_none())
        else {
            return Ok(false);
        };
        if current.obligations != run.obligations || current.gate != run.gate {
            return Ok(false);
        }
        current.obligations = next.clone();
        Ok(true)
    })?;
    let Some(task) = written else {
        return Ok(None);
    };
    for detail in transitions {
        transition(ctx, &task.id, "wait_transition", detail)?;
    }
    crate::parked::reacquire(ctx, &task.id)?;
    Ok(Some(Change {
        task: task.id,
        from: task.status,
        to: task.status,
        note: next.lines().join("; "),
        stage: None,
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
