//! Handlers of the reviewer (T-252 reviewer-role): evidence, artifact and verdict. Each admits the
//! request (bounds, the named review child, authentication, receipt lookup and, for a new
//! mutation, liveness) and then calls the existing corral operation, which commits the receipt
//! in its own final transaction. The launcher bound the parent run, the review child and the
//! callback token; the model never names them, so the verdict is always checked against the
//! attempt this host was launched for.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{AgentGateRef, RequestOrigin};
use corral_core::{ArtifactKind, EvidenceObservation, Verdict};
use serde::Deserialize;

use super::super::admission::{self, Admission, Mode};
use super::super::call::{AgentOp, CallResponse, RejectCode, Rejection};
use super::super::effects::{Receipted, Report};
use super::super::envelope::Envelope;
use super::worker::{ArtifactPayload, admitted, answer, artifact_path, origin_of};
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

/// Where one request stages a review child's own proof before it is attached. The review packet
/// treats an artifact whose source lies under `review-watch` as reviewer-side, so a reviewer's
/// proof never changes the candidate packet or evidence its own verdict is bound to.
fn proof_dir(ctx: &Ctx, (task, review): (&str, &str), origin: &RequestOrigin) -> PathBuf {
    let watch = ctx.store.state_dir().join("review-watch");
    watch
        .join(task)
        .join(format!("{review}-proof"))
        .join(&origin.request_id)
}

/// Attaches the staged file as reviewer-side proof of the parent run, then removes the staging
/// directory; the artifact keeps its own snapshot. A replay reaches the stored receipt before
/// the file is read, so it stages nothing.
fn attach_proof(
    ctx: &Ctx,
    task: &str,
    staged: &Path,
    (kind, title): (Option<ArtifactKind>, &str),
    origin: &RequestOrigin,
) -> Result<Receipted> {
    let attached =
        crate::artifacts::add_artifact_request(ctx, task, staged, kind, Some(title), origin);
    if let Some(dir) = staged.parent() {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", dir.display()));
            }
        }
    }
    attached
}

fn create(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))
}

/// corral_artifact: the file is copied into the request's proof directory and attached from
/// there, so it is reviewer-side proof rather than a new candidate artifact.
fn artifact(
    ctx: &Ctx,
    (task, run, review): (&str, &str, &str),
    input: ArtifactPayload,
    kind: Option<ArtifactKind>,
    (origin, replay): (&RequestOrigin, bool),
) -> Result<Receipted> {
    let path = artifact_path(ctx, task, run, input.path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| bad(format!("artifact {} has no file name", path.display())))?;
    let staged = proof_dir(ctx, (task, review), origin).join(&name);
    let title = input.title.unwrap_or_else(|| name.clone());
    if replay {
        return attach_proof(ctx, task, &staged, (kind, &title), origin);
    }
    let meta = std::fs::metadata(&path)
        .map_err(|_| bad(format!("artifact {} not found", path.display())))?;
    if !meta.is_file() {
        return Err(bad(format!(
            "artifact {} is not a regular file",
            path.display()
        )));
    }
    let max = ctx.store.config.artifacts.max_bytes;
    if meta.len() > max {
        return Err(bad(format!(
            "artifact {} is {} bytes, larger than max_bytes {max}",
            path.display(),
            meta.len()
        )));
    }
    create(staged.parent().context("the proof file has a directory")?)?;
    std::fs::copy(&path, &staged)
        .with_context(|| format!("copying {} to {}", path.display(), staged.display()))?;
    attach_proof(ctx, task, &staged, (kind, &title), origin)
}

/// corral_evidence: a reviewer's observation is validated against the task's criteria and the
/// parent run, then recorded as a reviewer-side evidence artifact. It is never appended to the
/// worker's criterion evidence, whose digest the review is bound to.
fn evidence(
    ctx: &Ctx,
    (task, run, review): (&str, &str, &str),
    observation: &EvidenceObservation,
    (origin, replay): (&RequestOrigin, bool),
) -> Result<Receipted> {
    let staged = proof_dir(ctx, (task, review), origin).join("evidence.json");
    let title = format!(
        "Reviewer evidence {} ({})",
        observation.criterion, observation.key
    );
    let kind = Some(ArtifactKind::Evidence);
    if replay {
        return attach_proof(ctx, task, &staged, (kind, &title), origin);
    }
    if observation.version != 1 {
        return Err(bad("invalid_evidence: an EvidenceObservation is version 1"));
    }
    if observation.run != run {
        return Err(bad(format!("wrong_run: the reviewed run is {run}")));
    }
    let saved = ctx.store.get(task)?;
    let manifest = saved
        .completion
        .as_ref()
        .ok_or_else(|| bad("missing_manifest: the task registers no criteria"))?;
    if !manifest
        .criteria
        .iter()
        .any(|c| c.id == observation.criterion)
    {
        return Err(bad(format!("unknown_criterion: {}", observation.criterion)));
    }
    create(staged.parent().context("the proof file has a directory")?)?;
    let mut text = serde_json::to_string_pretty(observation)?;
    text.push('\n');
    std::fs::write(&staged, text).with_context(|| format!("writing {}", staged.display()))?;
    attach_proof(ctx, task, &staged, (kind, &title), origin)
}

/// Admits a new mutation, reporting whether the request is a replay of a stored receipt.
fn admit(ctx: &Ctx, envelope: &Envelope) -> Result<(RequestOrigin, bool)> {
    let mode = Mode::Mutation { settles_turn: None };
    Ok(match admission::admit(ctx, envelope, mode)? {
        Admission::Replay(_) => (origin_of(envelope), true),
        Admission::New(admitted) => (admitted.origin, false),
    })
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
    let (Some(task), Some(run), Some(review), Some(callback)) = (
        identity.task.as_deref(),
        identity.run.as_deref(),
        identity.review.as_deref(),
        identity.callback.as_deref(),
    ) else {
        return Err(bad(
            "a reviewer request names its task, parent run, review child and callback token",
        ));
    };
    match op {
        AgentOp::Evidence => {
            let input: EvidenceObservation =
                super::payload(envelope, "an EvidenceObservation v1 object")?;
            let (origin, replay) = admit(ctx, envelope)?;
            evidence(ctx, (task, run, review), &input, (&origin, replay)).map(answer)
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
            let (origin, replay) = admit(ctx, envelope)?;
            artifact(ctx, (task, run, review), input, kind, (&origin, replay)).map(answer)
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
            let origin = admitted(ctx, envelope, Mode::Mutation { settles_turn: None })?;
            verdict(ctx, task, run, callback, &origin, input).map(answer)
        }
        other => Err(super::unavailable(other, OWNER)),
    }
}
