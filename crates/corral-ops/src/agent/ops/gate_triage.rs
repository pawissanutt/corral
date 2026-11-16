//! Authenticated, record-only attribution. The PM records directly; assistants use delegated
//! scope or one host decision. No attribution path queues pipeline work.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentRole, RESULT_MAX_BYTES, RequestIdentity};
use corral_core::{Store, TaskFieldsRef as _, TaskRef as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::{
    admission::{self, Admission, Mode},
    call::{CallResponse, RejectCode, Rejection},
    envelope::{Envelope, tool_request_id},
    receipts::{self, Commit},
};
use crate::gate_triage::{AREA, GateDiagnosis, GateTriage, prepare};
use crate::{Ctx, verification};

const PROPOSALS: &str = "gate-triage-proposals";

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Propose {
        task: String,
        diagnosis: GateDiagnosis,
    },
    Decide {
        proposal: String,
        digest: String,
        approved: bool,
    },
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    run: String,
    head: String,
    tree: String,
    spec: String,
    policy: String,
    task_state: String,
    triage: String,
}

#[derive(Serialize, Deserialize)]
struct Bound {
    proposal: String,
    identity: RequestIdentity,
    task: String,
    scope: Snapshot,
    report: GateDiagnosis,
    diagnosis: Value,
}

#[derive(Serialize, Deserialize)]
struct Proposal {
    bound: Bound,
    digest: String,
    decision: Option<bool>,
}

fn reject(code: RejectCode, reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

fn refused(error: anyhow::Error) -> anyhow::Error {
    if error.is::<Rejection>() {
        error
    } else {
        reject(RejectCode::FixedRule, format!("{error:#}"))
    }
}

fn hash(value: &impl Serialize) -> Result<String> {
    Ok(verification::digest(&serde_json::to_vec(value)?))
}

fn snapshot(
    ctx: &Ctx,
    head: &corral_core::TaskHead,
    run: &corral_core::RunHead,
    state: &GateTriage,
) -> Result<Snapshot> {
    ensure!(
        corral_git::is_clean(&run.worktree)?,
        "diagnosis candidate worktree is not clean"
    );
    Ok(Snapshot {
        run: run.id.clone(),
        head: corral_git::rev_sha(&run.worktree, "HEAD")?,
        tree: corral_git::rev_sha(&run.worktree, "HEAD^{tree}")?,
        spec: verification::spec_digest(head)?,
        policy: ctx.store.effective_operating_policy()?.revision,
        task_state: hash(&(
            head.contract_revision()?,
            head.status(),
            head.cowork(),
            head.rounds(),
            run,
        ))?,
        triage: hash(state)?,
    })
}

fn scope(ctx: &Ctx, envelope: &Envelope, head: &corral_core::TaskHead) -> Result<bool> {
    ensure!(
        crate::agent::hooks::pm::contains(
            &ctx.store,
            &envelope.identity.project,
            head.project.as_deref(),
        )?,
        "task is outside the caller's project scope"
    );
    if let Some((session, _)) = head
        .cowork_run()
        .and_then(|cowork| cowork.assistant_driver())
    {
        ensure!(
            envelope.identity.assistant.as_deref() == Some(session),
            "diagnosis belongs to the current assistant driver"
        );
    }
    crate::operating::attribution_scope(&ctx.store, head, envelope.identity.role == AgentRole::Pm)
}

fn owner_required(state: &GateTriage, report: &GateDiagnosis) -> bool {
    state
        .failures
        .iter()
        .find(|entry| {
            entry
                .gate
                .identity
                .as_ref()
                .is_some_and(|identity| identity.attempt == report.attempt)
        })
        .is_some_and(|entry| !entry.diagnoses.is_empty() && !entry.diagnoses.contains(report))
}

fn record(ctx: &Ctx, id: &str, mut state: GateTriage, report: &GateDiagnosis) -> Result<Value> {
    let entry = state
        .failures
        .iter_mut()
        .find(|entry| {
            entry
                .gate
                .identity
                .as_ref()
                .is_some_and(|identity| identity.attempt == report.attempt)
        })
        .context("diagnosis attempt is not a retained failed receipt")?;
    if !entry.diagnoses.contains(report) {
        entry.diagnoses.push(report.clone());
        entry.resolved_by = None;
        // Do not touch status, autopilot, retry_requested, diagnostic credit/history,
        // rounds, owner investigations or any failed/passing receipt.
        ctx.store.save_record(AREA, id, &state)?;
        ctx.store
            .log_event(Some(id), "gate_diagnosis", serde_json::to_value(report)?)?;
    }
    Ok(json!({"outcome":"recorded", "task":id, "attempt":report.attempt}))
}

fn proposed(bound: &Bound, digest: &str) -> Value {
    let summary = format!(
        "Record attribution only for task {:?}, run {:?}, candidate {:?} (tree {:?}), retained failed attempt {:?}. Class: {:?}. Explanation: {:?}. Evidence paths: {:?}. Baseline receipt: {:?}. This does not run a gate, grant PASS or a waiver, renew a retry, launch work, or authorize acceptance. Existing executor authority and full validation remain required.",
        bound.task,
        bound.scope.run,
        bound.scope.head,
        bound.scope.tree,
        bound.report.attempt,
        bound.report.class,
        bound.report.explanation,
        bound.report.evidence,
        bound.report.baseline,
    );
    json!({"outcome":"proposed", "proposal":bound.proposal, "digest":digest,
        "task":bound.task, "run":bound.scope.run, "head":bound.scope.head,
        "attempt":bound.report.attempt, "diagnosis":bound.diagnosis,
        "summary":summary, "confirmation_required":true})
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let action: Action = serde_json::from_value(envelope.payload.clone())
        .map_err(|_| reject(RejectCode::BadRequest, "expected action propose with task/diagnosis, or host-only decide with proposal/digest/approved"))?;
    if let Action::Propose { diagnosis, .. } = &action {
        ensure!(
            !diagnosis.retry,
            reject(
                RejectCode::BadRequest,
                "gate_triage is attribution-only; retry renewal is not supported"
            )
        );
    }
    if let Action::Decide { proposal, .. } = &action {
        let expected = tool_request_id(
            &envelope.identity.session_id,
            &format!(
                "gate-triage-decision:{}:{}",
                envelope.identity.generation, proposal
            ),
        );
        if envelope.identity.turn.is_some()
            || envelope.identity.parent.is_some()
            || envelope.request_id != expected
        {
            return Err(reject(
                RejectCode::NotOwnerInput,
                "gate_triage decide is a host-only confirmation, not a model action",
            ));
        }
    }
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => {
            return receipts::respond(
                ctx,
                &admission::container_of(&envelope.identity),
                *stored,
                true,
            );
        }
        Admission::New(admitted) => admitted,
    };
    let task = match &action {
        Action::Propose { task, .. } => Store::normalize_id(task).map_err(refused)?,
        Action::Decide { proposal, .. } => {
            ctx.store
                .record::<Proposal>(PROPOSALS, proposal)?
                .ok_or_else(|| reject(RejectCode::BadRequest, "gate triage proposal not found"))?
                .bound
                .task
        }
    };
    let _pilot = ctx
        .store
        .try_lock(&format!("autopilot-{task}"))?
        .ok_or_else(|| {
            reject(
                RejectCode::ConflictInFlight,
                "autopilot is advancing this task",
            )
        })?;
    let apply = || {
        ctx.store.read_locked_head(&task, |head| {
        let delegated = scope(ctx, envelope, head).map_err(refused)?;
        let result = match action {
            Action::Propose { diagnosis: report, .. } => {
                let (run, state) = prepare(ctx, head, &report).map_err(refused)?;
                if delegated && (envelope.identity.role == AgentRole::Pm || !owner_required(&state, &report)) {
                    record(ctx, &task, state, &report)?
                } else {
                    let bound = Bound {
                        proposal: envelope.request_id.clone(), identity: envelope.identity.clone(),
                        task: task.clone(), scope: snapshot(ctx, head, &run, &state).map_err(refused)?,
                        report, diagnosis: envelope.payload["diagnosis"].clone(),
                    };
                    let digest = hash(&bound)?;
                    let result = proposed(&bound, &digest);
                    ensure!(serde_json::to_vec(&result)?.len() <= RESULT_MAX_BYTES,
                        reject(RejectCode::BadRequest, "attribution proposal exceeds receipt result limit"));
                    ctx.store.save_record(PROPOSALS, &envelope.request_id, &Proposal { bound, digest, decision: None })?;
                    result
                }
            }
            Action::Decide { proposal, digest, approved } => {
                let mut saved: Proposal = ctx.store.record(PROPOSALS, &proposal)?
                    .ok_or_else(|| reject(RejectCode::BadRequest, "gate triage proposal not found"))?;
                let bound = &saved.bound;
                let caller = &envelope.identity;
                ensure!(bound.identity.store == caller.store && bound.identity.project == caller.project
                    && bound.identity.role == caller.role && bound.identity.assistant == caller.assistant
                    && bound.identity.session_id == caller.session_id && bound.identity.generation == caller.generation,
                    reject(RejectCode::NotOwnerInput, "proposal belongs to another session/generation/project"));
                ensure!(saved.digest == digest && hash(bound)? == digest,
                    reject(RejectCode::PayloadMismatch, "proposal digest does not match the exact attribution"));
                ensure!(saved.decision.is_none(), reject(RejectCode::StaleRevision, "proposal already decided"));
                let (run, state) = prepare(ctx, head, &bound.report).map_err(refused)?;
                ensure!(snapshot(ctx, head, &run, &state).map_err(refused)? == bound.scope,
                    reject(RejectCode::StaleRevision, "diagnosis task/candidate/spec/policy/prior attribution changed; proposal is stale"));
                let result = if approved { record(ctx, &task, state, &bound.report)? } else {
                    json!({"outcome":"declined", "task":task, "attempt":bound.report.attempt})
                };
                saved.decision = Some(approved);
                ctx.store.save_record(PROPOSALS, &proposal, &saved)?;
                ctx.store.log_event(
                    Some(&task),
                    "gate_triage_decision",
                    json!({
                        "proposal": proposal,
                        "digest": digest,
                        "approved": approved,
                        "request": envelope.request_id,
                        "session": caller.session_id,
                        "generation": caller.generation,
                        "attempt": saved.bound.report.attempt,
                    }),
                )?;
                result
            }
        };
        Ok(Commit { result, effect: None, post_pending: Vec::new() })
    })
    };
    if envelope.identity.role == AgentRole::Assistant {
        receipts::commit_assistant(ctx, &admitted, |assistant| {
            crate::assistant::workflow::boundary::require_ordinary(
                &ctx.store,
                envelope.identity.assistant.as_deref().unwrap_or_default(),
                assistant,
            )
            .map_err(refused)?;
            apply()
        })
    } else {
        receipts::commit(ctx, &admitted, |_, _| apply())
    }
}

#[cfg(test)]
mod tests;
