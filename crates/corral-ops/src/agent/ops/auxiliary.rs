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
use corral_core::agent::{
    AuxiliaryOutcome, ReceiptEffect, UsageSession, UsageSessionKind, WebSearchProvider, paths,
};
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
    #[serde(default)]
    plan_review: Option<crate::assistant::workflow::review::Association>,
    #[serde(default)]
    review_event: Option<crate::assistant::workflow::review::Event>,
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
    let snapshot = admitted.resolved.generation(&container, generation)?;
    if payload.plan_review.is_none() && payload.review_event.is_some() {
        return Err(bad("review lifecycle requires its exact association"));
    }
    // Settlement/recovery must remain possible after capability reload. Only claim and
    // provider accounting admission require subagents; they never enable it themselves.
    if let Some(association) = &payload.plan_review {
        use crate::assistant::workflow::review;
        let session = envelope
            .identity
            .assistant
            .as_deref()
            .ok_or_else(|| bad("local plan reviews are assistant-only"))?;
        if payload.usage_kind != Some(UsageSessionKind::Explore) {
            return Err(bad("local review must use explore accounting"));
        }
        let available = snapshot.as_ref().is_some_and(|g| g.capability("subagents"));
        if (!matches!(payload.review_event, Some(ref event) if !matches!(event, review::Event::Claim))
            && payload.action != "finish")
            && !available
        {
            return Err(Rejection::new(
                RejectCode::OpNotAllowed,
                "local review explore capability disabled",
            )
            .into());
        }
        let max = snapshot
            .as_ref()
            .and_then(|g| g.capabilities.as_ref())
            .and_then(|s| s.features.get("subagents"))
            .and_then(|f| f.settings.get("max_parallel"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;
        return receipts::commit_assistant(ctx, &admitted, |container| {
            let result = if let Some(event) = payload.review_event {
                if payload.action != "review"
                    || payload.model.is_some()
                    || payload.outcome.is_some()
                {
                    return Err(bad("invalid review lifecycle payload"));
                }
                review::apply_event(
                    &ctx.store,
                    session,
                    container,
                    generation,
                    association,
                    &payload.request,
                    event,
                )?
            } else {
                review::accounting(
                    &ctx.store,
                    session,
                    container,
                    generation,
                    association,
                    &payload.request,
                    review::Accounting {
                        action: &payload.action,
                        model: payload.model.as_deref(),
                        outcome: payload.outcome,
                        max,
                    },
                )?
            };
            Ok(Commit {
                result,
                effect: None,
                post_pending: Vec::new(),
            })
        });
    }
    let feature = match payload.usage_kind {
        Some(UsageSessionKind::Explore) => "subagents",
        Some(UsageSessionKind::Oracle) => "oracle",
        Some(UsageSessionKind::Shadow)
            if envelope.identity.role == corral_core::agent::AgentRole::Pm =>
        {
            "shadow"
        }
        Some(UsageSessionKind::WebSearch) => "web.enabled",
        _ => {
            return Err(bad(
                "auxiliary supports only explore, oracle, PM shadow and web_search",
            ));
        }
    };
    if !snapshot.as_ref().is_some_and(|g| g.capability(feature)) {
        return Err(
            Rejection::new(RejectCode::OpNotAllowed, "auxiliary capability is disabled").into(),
        );
    }
    if payload.usage_kind == Some(UsageSessionKind::WebSearch)
        && !snapshot
            .as_ref()
            .and_then(|g| g.capabilities.as_ref())
            .and_then(|s| s.web.as_ref())
            .is_some_and(|web| web.search_provider == Some(WebSearchProvider::Subscription))
    {
        return Err(
            Rejection::new(RejectCode::OpNotAllowed, "subscription search is disabled").into(),
        );
    }
    let max = snapshot
        .as_ref()
        .and_then(|g| g.capabilities.as_ref())
        .and_then(|s| s.features.get(feature))
        .and_then(|f| f.settings.get("max_parallel"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;
    receipts::commit_selected(
        ctx,
        &admitted,
        &receipts::pm_selection(&admitted.origin).family("usage_sessions"),
        move |state, _| {
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
                        .filter(|m| {
                            !m.is_empty() && m.len() <= 256 && !m.chars().any(char::is_control)
                        })
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
        },
    )
}

#[cfg(test)]
#[path = "auxiliary_shadow_tests.rs"]
mod shadow_tests;
