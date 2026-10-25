//! Handlers of the reviewer (T-252 reviewer-role): evidence, artifact and verdict. Each admits the
//! request (bounds, the named review child, authentication, receipt lookup and, for a new
//! mutation, liveness) and then calls the existing corral operation, which commits the receipt
//! in its own final transaction. The launcher bound the parent run, the review child and the
//! callback token; the model never names them, so the verdict is always checked against the
//! attempt this host was launched for.
use anyhow::Result;
use corral_core::agent::{AgentGateRef, RequestOrigin};
use corral_core::{ArtifactKind, EvidenceObservation, Verdict};
use serde::Deserialize;

use super::super::admission::Mode;
use super::super::call::{AgentOp, CallResponse, RejectCode, Rejection};
use super::super::effects::{Receipted, Report};
use super::super::envelope::Envelope;
use super::worker::{ArtifactPayload, admitted, answer, artifact_path};
use crate::Ctx;

/// The T-252 child that lands these handlers.
pub const OWNER: &str = "reviewer-role";

/// Whether this module handles `op` yet; hello advertises only landed ops.
pub(crate) fn lands(op: AgentOp) -> bool {
    matches!(op, AgentOp::Evidence | AgentOp::Artifact | AgentOp::Verdict)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerdictPayload {
    verdict: Verdict,
    notes: String,
    #[serde(default)]
    gate_ref: Option<AgentGateRef>,
    #[serde(default)]
    contract_sha256: Option<String>,
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

/// The refusal prefixes of the existing verdict path that answer as rejections, each with the
/// code the host acts on. The reason keeps the verbatim text from the prefix on.
const REFUSALS: &[(&str, RejectCode)] = &[
    ("review_questions_pending:", RejectCode::QuestionState),
    ("review_answers_unobserved:", RejectCode::QuestionState),
    ("stale review:", RejectCode::ReviewClosed),
    ("gate_ref_stale:", RejectCode::BadRequest),
];

/// corral_verdict: `record_verdict` commits the receipt and the VerdictReport in its update. A
/// refusal of the existing verdict binding (pending questions, unobserved answers, a stale
/// attempt or candidate) answers as a rejection, which `call` records, so the review stays open.
fn verdict(
    ctx: &Ctx,
    task: &str,
    run: &str,
    callback: &str,
    origin: &RequestOrigin,
    payload: VerdictPayload,
) -> Result<Receipted> {
    let contract_sha256 = match payload.contract_sha256 {
        Some(digest) => Some(digest),
        None => crate::agent::hooks::review::gate_section(ctx, task, run)?
            .map(|section| section.contract_sha256),
    };
    let report = Report {
        origin,
        gate_ref: payload.gate_ref,
        contract_sha256,
    };
    let notes = payload.notes.as_str();
    let attempt = Some(callback);
    match crate::reviewer::verdict_request(ctx, task, payload.verdict, notes, attempt, &report) {
        Err(error) if error.downcast_ref::<Rejection>().is_none() => {
            let reason = format!("{error:#}");
            let refused = REFUSALS
                .iter()
                .filter_map(|(prefix, code)| reason.find(prefix).map(|at| (at, *code)))
                .min_by_key(|(at, _)| *at);
            match refused {
                Some((at, code)) => Err(Rejection::new(code, &reason[at..]).into()),
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
    let (Some(task), Some(run), Some(_), Some(callback)) = (
        identity.task.as_deref(),
        identity.run.as_deref(),
        identity.review.as_deref(),
        identity.callback.as_deref(),
    ) else {
        return Err(bad(
            "a reviewer request names its task, parent run, review child and callback token",
        ));
    };
    let mutation = Mode::Mutation { settles_turn: None };
    match op {
        AgentOp::Evidence => {
            let input: EvidenceObservation =
                super::payload(envelope, "an EvidenceObservation v1 object")?;
            let origin = admitted(ctx, envelope, mutation)?;
            crate::completion::record_evidence_request(ctx, task, &input, &origin).map(answer)
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
            // The review child works in its parent run's worktree.
            let path = artifact_path(ctx, task, run, input.path)?;
            let title = input.title.as_deref();
            crate::artifacts::add_artifact_request(ctx, task, &path, kind, title, &origin)
                .map(answer)
        }
        AgentOp::Verdict => {
            let input: VerdictPayload = super::payload(
                envelope,
                "verdict (approve|changes|escalate), notes, gate_ref? {attempt, tier, \
                 receipt_sha256}, contract_sha256?",
            )?;
            if input.notes.trim().is_empty() {
                return Err(bad("a verdict needs notes"));
            }
            let origin = admitted(ctx, envelope, mutation)?;
            verdict(ctx, task, run, callback, &origin, input).map(answer)
        }
        other => Err(super::unavailable(other, OWNER)),
    }
}
