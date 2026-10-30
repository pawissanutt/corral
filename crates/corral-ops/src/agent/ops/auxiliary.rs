//! Host-only auxiliary lifecycle events. The container, generation and transcript path come
//! from admission/path helpers, never from a child or a model-selected filesystem path.
use crate::{
    Ctx,
    agent::{
        admission::{self, Admission, Mode},
        call::{CallResponse, RejectCode, Rejection},
        envelope::{Envelope, event_id_parts},
        receipts::{self, Commit},
    },
};
use anyhow::Result;
use corral_core::agent::{AuxiliaryOutcome, ReceiptEffect, UsageSession, UsageSessionKind, paths};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    kind: String,
    action: String,
    request: String,
    #[serde(default)]
    usage_kind: Option<UsageSessionKind>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    outcome: Option<AuxiliaryOutcome>,
}
fn bad(message: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, message).into()
}
fn word(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
pub(super) fn auxiliary(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: Payload = super::payload(
        envelope,
        "kind=auxiliary, action, request, usage_kind?, model?, outcome?",
    )?;
    if payload.kind != "auxiliary"
        || event_id_parts(&envelope.request_id).is_none()
        || !word(&payload.request)
    {
        return Err(bad(
            "auxiliary needs a lifecycle request ID and a bounded invocation key",
        ));
    }
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let generation = envelope.identity.generation;
    let file = paths::usage_session_file(&ctx.store, &container, &payload.request);
    let snapshot = admitted
        .resolved
        .state(&container)
        .and_then(|s| s.current_generation());
    let feature = match payload.usage_kind {
        Some(UsageSessionKind::Explore) => "subagents",
        Some(UsageSessionKind::Oracle) => "oracle",
        _ => return Err(bad("auxiliary supports only explore and oracle")),
    };
    if !snapshot.is_some_and(|g| g.capability(feature)) {
        return Err(
            Rejection::new(RejectCode::OpNotAllowed, "auxiliary capability is disabled").into(),
        );
    }
    let max = snapshot
        .and_then(|g| g.capabilities.as_ref())
        .and_then(|s| s.features.get(feature))
        .and_then(|f| f.settings.get("max_parallel"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;
    receipts::commit(ctx, &admitted, move |state, _| {
        let existing = state
            .usage_sessions
            .iter()
            .position(|s| s.request.as_deref() == Some(&payload.request));
        match payload.action.as_str() {
            "prepare" => {
                if existing.is_some()
                    || payload.outcome.is_some()
                    || state.usage_sessions.len() >= 4096
                {
                    return Err(bad(
                        "duplicate/invalid auxiliary prepare or auxiliary record limit reached",
                    ));
                }
                let model = payload
                    .model
                    .as_deref()
                    .filter(|m| !m.is_empty() && m.len() <= 256 && !m.chars().any(char::is_control))
                    .ok_or_else(|| bad("prepare needs its configured model"))?;
                state.usage_sessions.push(UsageSession {
                    generation,
                    kind: payload
                        .usage_kind
                        .clone()
                        .ok_or_else(|| bad("missing kind"))?,
                    session_id: payload.request.clone(),
                    session_file: file.clone(),
                    model: Some(model.into()),
                    request: Some(payload.request.clone()),
                    outcome: Some(AuxiliaryOutcome::Prepared),
                });
            }
            "started" | "finish" => {
                let index = existing.ok_or_else(|| bad("unknown auxiliary invocation"))?;
                let live = state
                    .usage_sessions
                    .iter()
                    .filter(|s| {
                        s.generation == generation
                            && s.kind
                                == payload
                                    .usage_kind
                                    .clone()
                                    .unwrap_or(UsageSessionKind::Summary)
                            && s.outcome == Some(AuxiliaryOutcome::Running)
                    })
                    .count();
                let record = &mut state.usage_sessions[index];
                if record.generation != generation
                    || Some(&record.kind) != payload.usage_kind.as_ref()
                {
                    return Err(bad("auxiliary belongs to another generation/kind"));
                }
                if payload.action == "started" {
                    if record.outcome != Some(AuxiliaryOutcome::Prepared) || live >= max {
                        return Err(bad("auxiliary parallel limit or invalid start"));
                    }
                    record.outcome = Some(AuxiliaryOutcome::Running);
                } else {
                    let outcome = payload
                        .outcome
                        .as_ref()
                        .ok_or_else(|| bad("finish needs outcome"))?;
                    let allowed = match record.outcome {
                        Some(AuxiliaryOutcome::Prepared) => {
                            *outcome == AuxiliaryOutcome::NotStarted
                        }
                        Some(AuxiliaryOutcome::Running) => matches!(
                            outcome,
                            AuxiliaryOutcome::Completed | AuxiliaryOutcome::Uncertain
                        ),
                        _ => false,
                    };
                    if !allowed {
                        return Err(bad("auxiliary outcome is terminal or invalid"));
                    }
                    record.outcome = Some(outcome.clone());
                }
            }
            _ => return Err(bad("unknown auxiliary action")),
        }
        Ok(Commit {
            result: json!({"session_file": file, "session_id": payload.request, "generation": generation}),
            effect: Some(ReceiptEffect {
                kind: "auxiliary".into(),
                reference: payload.request,
            }),
            post_pending: Vec::new(),
        })
    })
}
