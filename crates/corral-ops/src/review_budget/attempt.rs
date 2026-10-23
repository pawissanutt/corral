//! The evidence of one auxiliary attempt, read only through the existing accessors: the
//! claim reference and its authorized-submission disposition, the launch journal and the
//! retained preparation. It never builds a path, so a storage conversion keeps it correct.
use std::time::Duration;

use anyhow::{Context, Result};
use corral_core::agent::{
    self, AgentRunState, ContainerRef, EVIDENCE_MAX, Executor, ReceiptEvidence, ReceiptState,
    RefusalCode, RequestBinding, RequestReceipt, WakeKind, WakeState,
};
use corral_core::{GateResult, Store, SubmissionDisposition, Task};
use jiff::Timestamp;
use serde_json::{Value, json};

use super::{Kind, Pending, load_pending, submission};
use crate::{Ctx, operating};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttemptState {
    Absent,
    /// Authorized, never submitted. A refusal not attributable to the request is kept as
    /// evidence about the attempt, never as that request's refusal.
    Prepared {
        prior_refusal: Option<String>,
    },
    /// A durable refusal attributable to the request, or any refusal when none is named.
    Refused {
        reason: String,
    },
    Submitting,
    Submitted,
    Uncertain {
        reason: String,
    },
}

fn missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    })
}

/// A refusal counts for `request` when its verbatim reason carries `agent_request=<request>`
/// (written in the store's locked refusal) or the journal or retained preparation records
/// `refused_request == request` (the store's policy check runs before the closure).
pub(crate) fn attempt_state(
    ctx: &Ctx,
    task: &Task,
    run: &str,
    kind: Kind,
    attempt: &str,
    request: Option<&str>,
) -> Result<AttemptState> {
    let action = submission::action_kind(kind);
    let authorization = match operating::load(ctx, &task.id, run, action, attempt) {
        Ok(authorization) => authorization,
        Err(error) if missing(&error) => return Ok(AttemptState::Absent),
        Err(error) => return Err(error),
    };
    let disposition = ctx
        .store
        .authorized_submission(&authorization.id)?
        .disposition;
    let same = |p: &Pending| {
        p.task == task.id
            && p.run == run
            && p.kind == kind
            && p.authorization.as_ref().map(|a| a.id.as_str()) == Some(authorization.id.as_str())
    };
    let journal = load_pending(ctx)?.filter(same);
    let retained = submission::load_retained(ctx, &task.id, run, kind)?.filter(same);
    let records = || journal.iter().chain(retained.iter());
    let token = request.map(|r| format!("agent_request={r}"));
    let attributed = |reason: &str| match (request, &token) {
        (Some(request), Some(token)) => {
            reason.split(": ").any(|part| part == token)
                || records().any(|p| p.refused_request.as_deref() == Some(request))
        }
        _ => true,
    };
    Ok(match disposition {
        SubmissionDisposition::Submitted => AttemptState::Submitted,
        SubmissionDisposition::Submitting => AttemptState::Submitting,
        SubmissionDisposition::Uncertain { reason } => AttemptState::Uncertain { reason },
        SubmissionDisposition::AuthorizationRefused { reason } if attributed(&reason) => {
            AttemptState::Refused { reason }
        }
        SubmissionDisposition::AuthorizationRefused { reason } => AttemptState::Prepared {
            prior_refusal: Some(reason),
        },
        SubmissionDisposition::Prepared if journal.iter().any(|p| p.submitted == Some(true)) => {
            AttemptState::Uncertain {
                reason: "the launch journal marked a submission the store never recorded".into(),
            }
        }
        SubmissionDisposition::Prepared => AttemptState::Prepared {
            prior_refusal: records().find_map(|p| p.refused.clone()),
        },
    })
}

/// The gate attempt a new request of this run adopts: a retained preparation, or a stale
/// never-submitted journal that the next reservation retains.
pub(crate) fn retained_attempt(ctx: &Ctx, task: &str, run: &str) -> Result<Option<String>> {
    let stale = load_pending(ctx)?.filter(|p| {
        p.task == task
            && p.run == run
            && p.kind == Kind::Gate
            && p.started
            && p.submitted == Some(false)
    });
    let retained = match stale {
        Some(pending) => Some(pending),
        None => submission::load_retained(ctx, task, run, Kind::Gate)?,
    };
    Ok(retained.and_then(|p| p.authorization.map(|a| a.action.attempt)))
}

/// Largest refusal reason a receipt quotes; longer reasons are cut at a character boundary.
const REASON_MAX_BYTES: usize = 4096;

/// An armed gate wake due now: its attempt finished, or its start stayed uncertain longer
/// than `uncertain_after`. The inbox post belongs to the wake adapter; `fire_wake` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueWake {
    pub container: ContainerRef,
    pub request_id: String,
    pub reason: String,
}

pub fn due_gate_wakes(task: &Task, now: Timestamp, uncertain_after: Duration) -> Vec<DueWake> {
    let mut due = Vec::new();
    for run in &task.runs {
        let Some(state) = run.agent_host.as_deref() else {
            continue;
        };
        let armed = state
            .wakes
            .iter()
            .filter(|w| w.kind == WakeKind::Gate && w.state == WakeState::Armed);
        for wake in armed {
            let gate = run.gate.as_ref().filter(|g| {
                g.identity
                    .as_ref()
                    .is_some_and(|i| i.attempt == wake.target)
            });
            let receipt = state.receipt(&wake.request_id);
            let reason = match gate {
                Some(gate) if gate.process_lost => Some("lost".to_owned()),
                Some(gate) if gate.result != GateResult::Running => {
                    Some(gate.result.as_str().to_owned())
                }
                _ if receipt.is_some_and(|r| {
                    r.state == ReceiptState::Uncertain
                        && now.duration_since(r.at).unsigned_abs() >= uncertain_after
                }) =>
                {
                    Some("gate_start_uncertain".to_owned())
                }
                _ => None,
            };
            if let Some(reason) = reason {
                let container = ContainerRef::Worker {
                    task: task.id.clone(),
                    run: run.id.clone(),
                };
                due.push(DueWake {
                    container,
                    request_id: wake.request_id.clone(),
                    reason,
                });
            }
        }
    }
    due
}

/// Records one wake as fired, keyed `wake:<task>/<run>/<target>/<reason>`; false when it
/// already fired or was cancelled, so a wake posts at most once.
pub fn fire_wake(store: &Store, due: &DueWake, message_id: Option<String>) -> Result<bool> {
    let task_id = due
        .container
        .task()
        .context("gate wakes live on worker runs")?;
    let fired = store.try_update(task_id, |task| {
        let state = agent::container_state_mut(task, &due.container)
            .context("the agent binding disappeared")?;
        let Some(wake) = state
            .wakes
            .iter_mut()
            .find(|w| w.request_id == due.request_id && w.state == WakeState::Armed)
        else {
            return Ok(false);
        };
        wake.key = format!(
            "wake:{}/{}/{}",
            due.container.key(),
            wake.target,
            due.reason
        );
        wake.state = WakeState::Fired;
        wake.fired_at = Some(Timestamp::now());
        wake.message_id = message_id;
        Ok(true)
    })?;
    Ok(fired.is_some())
}

/// `ag-` plus the first 16 hex digits of the request ID's hash part.
pub fn own_attempt(request_id: &str) -> String {
    let hex: String = request_id
        .trim_start_matches("rq-")
        .chars()
        .take(16)
        .collect();
    format!("ag-{hex}")
}

pub(crate) fn bounded(reason: &str) -> String {
    let mut end = reason.len().min(REASON_MAX_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_owned()
}

/// Gate refusal normalization: codes come from tokens in the verbatim reason, never prefixes.
fn refusal_code(reason: &str, request: &str) -> String {
    let parts: Vec<&str> = reason.split(": ").collect();
    if let Some(code) = parts.iter().find_map(|p| p.strip_prefix("agent_refusal=")) {
        code.to_owned()
    } else if parts.contains(&format!("agent_request={request}").as_str()) {
        "precondition_refused".into()
    } else {
        "policy_refused".into()
    }
}

fn binding_of(receipt: &RequestReceipt) -> RequestBinding {
    RequestBinding {
        container: ContainerRef::Worker {
            task: receipt.identity.task.clone().unwrap_or_default(),
            run: receipt.identity.run.clone().unwrap_or_default(),
        },
        generation: receipt.identity.generation,
        callback: receipt.identity.callback.clone(),
        settles_turn: None,
    }
}

/// The attempt a gate receipt names and its evidence; the request's own attempt is the
/// fallback when an adoption target was consumed before the launch bound it.
fn gate_evidence(
    ctx: &Ctx,
    task: &Task,
    receipt: &RequestReceipt,
) -> Result<(String, AttemptState)> {
    let run = receipt
        .identity
        .run
        .as_deref()
        .context("gate receipt names no run")?;
    let own = own_attempt(&receipt.request_id);
    let named = receipt
        .effect
        .as_ref()
        .map_or_else(|| own.clone(), |e| e.reference.clone());
    let request = Some(receipt.request_id.as_str());
    let state = attempt_state(ctx, task, run, Kind::Gate, &named, request)?;
    if state != AttemptState::Absent || named == own {
        return Ok((named, state));
    }
    let fallback = attempt_state(ctx, task, run, Kind::Gate, &own, request)?;
    Ok(if fallback == AttemptState::Absent {
        (named, state)
    } else {
        (own, fallback)
    })
}

/// The gate receipt state, result and wake transition this attempt's evidence supports (F1.a).
/// An error the executor itself returned (`error`) explains an attempt that never started.
pub(crate) fn gate_receipt(
    ctx: &Ctx,
    task: &Task,
    receipt: &RequestReceipt,
    error: Option<&str>,
) -> Result<(ReceiptState, Value, Option<WakeState>)> {
    let (attempt, evidence) = gate_evidence(ctx, task, receipt)?;
    let live = agent::check_live(task, &binding_of(receipt));
    let binding = live.as_ref().map_or_else(|r| r.code.as_str(), |()| "live");
    let adopted = attempt != own_attempt(&receipt.request_id);
    let run = receipt.identity.run.as_deref().unwrap_or_default();
    let started = json!({"outcome": "started", "attempt": attempt, "adopted": adopted});
    let ran = || crate::gate::attempt_started(ctx, &task.id, run, &attempt);
    use ReceiptState::{Abandoned, Committed, Rejected, Reserved, Uncertain};
    let armed = Some(WakeState::Armed);
    let cancelled = Some(WakeState::Cancelled);
    Ok(match evidence {
        AttemptState::Submitted => (Committed, started, armed),
        AttemptState::Submitting | AttemptState::Uncertain { .. } if ran() => {
            (Committed, started, armed)
        }
        AttemptState::Submitting => (
            Uncertain,
            json!({"outcome": "uncertain", "attempt": attempt, "reason": "submission in flight"}),
            armed,
        ),
        AttemptState::Uncertain { reason } => (
            Uncertain,
            json!({"outcome": "uncertain", "attempt": attempt, "reason": bounded(&reason)}),
            armed,
        ),
        AttemptState::Refused { reason } => (
            Rejected,
            json!({
                "outcome": "submission_refused", "attempt": attempt, "binding": binding,
                "code": refusal_code(&reason, &receipt.request_id), "reason": bounded(&reason),
            }),
            cancelled,
        ),
        AttemptState::Prepared { prior_refusal } => match &live {
            Err(r) if matches!(r.code, RefusalCode::AgentFenced | RefusalCode::RunEnded) => (
                Abandoned,
                json!({
                    "outcome": "not_submitted", "attempt": attempt, "binding": binding,
                    "prior_refusal": prior_refusal.as_deref().map(bounded),
                }),
                cancelled,
            ),
            _ => (
                Reserved,
                json!({
                    "outcome": "prepared", "attempt": attempt, "adopted": adopted,
                    "prior_refusal": prior_refusal.as_deref().map(bounded),
                }),
                None,
            ),
        },
        AttemptState::Absent => match error {
            Some(reason) => (
                Rejected,
                json!({"outcome": "not_started", "binding": binding, "reason": bounded(reason)}),
                cancelled,
            ),
            None => (
                Abandoned,
                json!({"outcome": "not_started", "binding": binding}),
                cancelled,
            ),
        },
    })
}

fn set_wake(state: &mut AgentRunState, request: &str, to: WakeState) {
    for wake in state.wakes.iter_mut().filter(|w| w.request_id == request) {
        if matches!(wake.state, WakeState::PendingStart | WakeState::Armed) {
            wake.state = to;
        }
    }
}

/// Settles a gate receipt inside one update of its task, from evidence read under the same
/// locks. `seen` is the executor the caller verified (itself, or gone); a receipt that moved
/// on since is returned unchanged, and a settled receipt is final.
pub(crate) fn settle(
    ctx: &Ctx,
    receipt: &RequestReceipt,
    seen: Option<&Executor>,
    by_executor: bool,
    error: Option<String>,
) -> Result<RequestReceipt> {
    let container = ContainerRef::Worker {
        task: receipt.identity.task.clone().unwrap_or_default(),
        run: receipt.identity.run.clone().unwrap_or_default(),
    };
    let task_id = receipt
        .identity
        .task
        .clone()
        .context("gate receipt names no task")?;
    let request = receipt.request_id.clone();
    let mut out = None;
    ctx.store.try_update(&task_id, |task| {
        let current = agent::container_state(task, &container)
            .and_then(|s| s.receipt(&request))
            .cloned()
            .context("gate receipt disappeared")?;
        let open = match current.state {
            ReceiptState::Reserved => current.executor.as_ref() == seen,
            ReceiptState::Uncertain => true,
            _ => false,
        };
        let (state, result, wake) = gate_receipt(ctx, task, &current, error.as_deref())?;
        let regress = current.state == ReceiptState::Uncertain && state != ReceiptState::Committed;
        if !open || regress || (state == current.state && result == current.result && !by_executor)
        {
            out = Some(current);
            return Ok(false);
        }
        let agent =
            agent::container_state_mut(task, &container).context("gate binding disappeared")?;
        let stored = agent.receipts.iter_mut().find(|r| r.request_id == request);
        let stored = stored.context("gate receipt disappeared")?;
        let now = Timestamp::now();
        if stored.result != result {
            let source = if by_executor {
                "executor"
            } else {
                "attempt_state"
            };
            let detail = error.as_deref().map(bounded);
            evidence(stored, &result, source, detail);
        }
        stored.state = state;
        stored.result = result;
        if state != ReceiptState::Reserved || by_executor {
            stored.executor = None;
        }
        if state == ReceiptState::Committed {
            stored.committed_at = Some(now);
        }
        let settled = stored.clone();
        if let Some(wake) = wake {
            set_wake(agent, &request, wake);
        }
        out = Some(settled);
        Ok(true)
    })?;
    out.context("gate settlement produced no receipt")
}

/// Appends one reconciled state, oldest first, keeping at most `EVIDENCE_MAX`.
pub(crate) fn evidence(
    stored: &mut RequestReceipt,
    result: &Value,
    source: &str,
    detail: Option<String>,
) {
    if stored.evidence.len() >= EVIDENCE_MAX {
        stored.evidence.remove(0);
    }
    stored.evidence.push(ReceiptEvidence {
        state: result["outcome"].as_str().unwrap_or_default().into(),
        source: source.into(),
        detail,
        at: Timestamp::now(),
    });
}
