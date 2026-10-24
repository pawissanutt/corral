//! Handlers of the worker (T-252 worker-role): wait, progress, checkpoint, evidence, artifact,
//! gate and done. Each admits the request (bounds, the named container, authentication, receipt
//! lookup and, for a new mutation, liveness) and then calls the existing corral operation, which
//! commits the receipt in its own final transaction (op-receipts). A replay reaches the same
//! operation, which answers with the stored receipt and completes any effect it still owes.
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::agent::{AgentGateRef, RequestBinding, RequestOrigin};
use corral_core::{
    ArtifactKind, CheckpointUpdate, EvidenceObservation, ObligationSource, ProgressUpdate,
    WaitHandle, WaitRequest,
};
use jiff::Timestamp;
use serde::Deserialize;

use super::super::admission::{self, Admission, Mode};
use super::super::call::{AgentOp, CallResponse, RejectCode, Rejection};
use super::super::effects::{self, Receipted, Report};
use super::super::envelope::Envelope;
use crate::Ctx;

/// The T-252 child that lands these handlers.
pub const OWNER: &str = "worker-role";

/// The refusal prefix of done while a question of the run is pending (OD6).
const OPEN_QUESTIONS: &str = "open_questions:";

/// Whether this module handles `op` yet; hello advertises only landed ops.
pub(crate) fn lands(op: AgentOp) -> bool {
    matches!(
        op,
        AgentOp::Wait
            | AgentOp::Progress
            | AgentOp::Checkpoint
            | AgentOp::Evidence
            | AgentOp::Artifact
            | AgentOp::Gate
            | AgentOp::Done
    )
}

/// corral_wait: the run comes from the binding, never from the model.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitPayload {
    id: String,
    condition: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    deadline: Option<Timestamp>,
    handle: WaitHandle,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactPayload {
    path: PathBuf,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GatePayload {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DonePayload {
    summary: String,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    gate_ref: Option<AgentGateRef>,
    #[serde(default)]
    contract_sha256: Option<String>,
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

/// The request as the operations receive it, built from the authenticated envelope: the same
/// origin admission derives for a new request, so a replay names the identical receipt.
fn origin_of(envelope: &Envelope) -> RequestOrigin {
    let identity = &envelope.identity;
    RequestOrigin {
        request_id: envelope.request_id.clone(),
        op: envelope.op.clone(),
        payload_sha256: envelope.payload_sha256.clone(),
        identity: identity.clone(),
        binding: RequestBinding {
            container: admission::container_of(identity),
            generation: identity.generation,
            callback: identity.callback.clone(),
            settles_turn: None,
        },
    }
}

fn answer(receipted: Receipted) -> CallResponse {
    CallResponse::Receipt {
        receipt: receipted.receipt,
        replayed: receipted.replayed,
    }
}

/// The bounded-observation capability the shared wait reconciler requires of a channel.
const WAIT_CAPABILITY: &str = "wait_v1";

/// Pi-native waits must be observable: a gate already wakes the worker through corral_gate, and
/// a channel without a deadline has no guaranteed wake (F5). The Pi-native host declares
/// `wait_v1` for every channel wait, because its deadline wake is the one corral guarantees.
fn wait_request(payload: WaitPayload, run: &str) -> Result<WaitRequest> {
    let handle = match payload.handle {
        WaitHandle::Gate { .. } => {
            return Err(bad(
                "gate waits are armed by corral_gate itself; wait on a channel instead",
            ));
        }
        WaitHandle::Channel { .. } if payload.deadline.is_none() => {
            return Err(bad(
                "unobservable_wait: a channel wait needs a deadline, the only wake corral can \
                 guarantee for it",
            ));
        }
        WaitHandle::Channel {
            capability: Some(other),
            ..
        } if other != WAIT_CAPABILITY => {
            return Err(bad(format!(
                "pi-native channel waits declare {WAIT_CAPABILITY}, not {other}"
            )));
        }
        WaitHandle::Channel { channel_id, .. } => WaitHandle::Channel {
            channel_id,
            capability: Some(WAIT_CAPABILITY.into()),
        },
    };
    Ok(WaitRequest {
        version: 1,
        id: payload.id,
        run: run.to_owned(),
        condition: payload.condition,
        owner: payload.owner.unwrap_or_else(|| "worker".into()),
        source: ObligationSource::Worker,
        deadline: payload.deadline,
        handle,
    })
}

fn artifact_path(ctx: &Ctx, task: &str, run: &str, path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let task = ctx.store.get(task)?;
    let worktree = task
        .runs
        .iter()
        .find(|r| r.id == run)
        .map(|r| r.worktree.clone())
        .context("the bound run disappeared")?;
    Ok(worktree.join(path))
}

/// done refused because a question is pending answers rejected with the keys, so the run
/// stays live and the worker either waits for the answer or withdraws (OD6).
fn done(ctx: &Ctx, task: &str, origin: &RequestOrigin, payload: DonePayload) -> Result<Receipted> {
    let report = Report {
        origin,
        gate_ref: payload.gate_ref,
        contract_sha256: payload.contract_sha256,
    };
    let result = payload.result.as_deref();
    match crate::review::done_request(ctx, task, &payload.summary, result, &report) {
        Err(error) if error.downcast_ref::<Rejection>().is_none() => {
            let reason = format!("{error:#}");
            match reason.find(OPEN_QUESTIONS) {
                Some(at) => Err(Rejection::new(RejectCode::QuestionState, &reason[at..]).into()),
                None => Err(error),
            }
        }
        other => other,
    }
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    if !lands(op) {
        return Err(super::unavailable(op, OWNER));
    }
    let identity = &envelope.identity;
    let (task, run) = match (identity.task.as_deref(), identity.run.as_deref()) {
        (Some(task), Some(run)) => (task.to_owned(), run.to_owned()),
        _ => return Err(bad("a worker request names its task and run")),
    };
    let mutation = Mode::Mutation { settles_turn: None };
    match op {
        AgentOp::Wait => {
            let payload: WaitPayload = super::payload(
                envelope,
                "id, condition, owner?, deadline?, handle {kind: channel, channel_id, capability?}",
            )?;
            let request = wait_request(payload, &run)?;
            let origin = admitted(ctx, envelope, mutation)?;
            let now = Timestamp::now();
            crate::obligations::register_wait_request(ctx, &task, request, now, &origin).map(answer)
        }
        AgentOp::Progress => {
            let input: ProgressUpdate = super::payload(envelope, "a ProgressUpdate v1 object")?;
            let origin = admitted(ctx, envelope, mutation)?;
            crate::progress::update_progress_request(ctx, &task, &input, &origin).map(answer)
        }
        AgentOp::Checkpoint => {
            let input: CheckpointUpdate = super::payload(envelope, "a CheckpointUpdate v1 object")?;
            let origin = admitted(ctx, envelope, mutation)?;
            crate::progress::save_checkpoint_request(ctx, &task, &input, &origin).map(answer)
        }
        AgentOp::Evidence => {
            let input: EvidenceObservation =
                super::payload(envelope, "an EvidenceObservation v1 object")?;
            let origin = admitted(ctx, envelope, mutation)?;
            crate::completion::record_evidence_request(ctx, &task, &input, &origin).map(answer)
        }
        AgentOp::Artifact => {
            let input: ArtifactPayload = super::payload(envelope, "path, kind?, title?")?;
            let kind = match input.kind.as_deref() {
                Some(name) => Some(
                    ArtifactKind::parse(name)
                        .ok_or_else(|| bad(format!("unknown artifact kind {name}")))?,
                ),
                None => None,
            };
            let origin = admitted(ctx, envelope, mutation)?;
            let path = artifact_path(ctx, &task, &run, input.path)?;
            let title = input.title.as_deref();
            crate::artifacts::add_artifact_request(ctx, &task, &path, kind, title, &origin)
                .map(answer)
        }
        AgentOp::Gate => {
            let _: GatePayload = super::payload(envelope, "none (an empty object)")?;
            // The reservation's own transaction checks liveness for a new request, and a replay
            // settles from attempt evidence, so admission only authenticates and looks up here.
            let origin = admitted(ctx, envelope, Mode::Lookup)?;
            effects::gate(ctx, &origin)
        }
        AgentOp::Done => {
            let input: DonePayload = super::payload(
                envelope,
                "summary, result?, gate_ref? {attempt, tier, receipt_sha256}, contract_sha256?",
            )?;
            let origin = admitted(ctx, envelope, mutation)?;
            done(ctx, &task, &origin, input).map(answer)
        }
        other => Err(super::unavailable(other, OWNER)),
    }
}

fn admitted(ctx: &Ctx, envelope: &Envelope, mode: Mode) -> Result<RequestOrigin> {
    Ok(match admission::admit(ctx, envelope, mode)? {
        Admission::Replay(_) => origin_of(envelope),
        Admission::New(admitted) => admitted.origin,
    })
}
