//! Receipt-backed shadow recording. Model choices never reach a board executor.
use super::super::{
    admission::{self, Admission, Mode},
    call::{CallResponse, RejectCode, Rejection},
    envelope::Envelope,
    receipts::{self, Commit},
};
use crate::{
    Ctx,
    pm_escalation::{self, EscalationContext},
};
use anyhow::Result;
use corral_core::agent::{
    AgentRole,
    pm_shadow::{self, PmShadowDecision},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    digest: String,
    choice: String,
    requested_escalation: bool,
    model: String,
    reason: Option<String>,
}

fn bad(reason: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

/// Add observational metadata only when it fits, never rebudgeting or changing the card.
/// Native card bounding changes presentation, not the identity fields hashed here.
pub(crate) fn with_brief_digest(
    mut brief: serde_json::Value,
    card: &crate::PmAction,
) -> Result<serde_json::Value> {
    brief["shadow_digest"] = json!(pm_escalation::shadow_digest(card));
    if serde_json::to_vec(&brief)?.len() > corral_core::agent::RESULT_MAX_BYTES
        && let Some(object) = brief.as_object_mut()
    {
        object.remove("shadow_digest");
    }
    Ok(brief)
}

pub(crate) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    if envelope.identity.role != AgentRole::Pm {
        return Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "shadow recording belongs to the PM",
        )
        .into());
    }
    let request: Request = super::payload(
        envelope,
        "digest, choice, requested_escalation, model, reason?",
    )?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let enabled = admitted
        .resolved
        .generation(&container, admitted.origin.binding.generation)?
        .is_some_and(|generation| generation.capability("shadow"));
    if !enabled {
        return Err(Rejection::new(
            RejectCode::OpNotAllowed,
            "shadow is disabled for this generation",
        )
        .into());
    }
    receipts::commit(ctx, &admitted, |_, _| {
        if request.digest.len() != 64 || !request.digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad("digest must be a 64-character hexadecimal SHA-256"));
        }
        if request.model.is_empty()
            || request.model.len() > 256
            || request.reason.as_ref().is_some_and(|r| r.len() > 500)
        {
            return Err(bad(
                "model must be 1..256 bytes and reason at most 500 bytes",
            ));
        }
        let project = envelope.identity.project.as_str();
        let projects = if project == super::super::hooks::pm::WORKSPACE {
            super::super::hooks::pm::projects(&ctx.store)?
        } else {
            [project.to_owned()].into()
        };
        let card = crate::pm_workset::next_for_projects(ctx, &projects)?;
        if request.choice != "escalate" && !card.options.iter().any(|o| o.id == request.choice) {
            return Err(bad("choice must name a current card option or escalate"));
        }
        let digest = pm_escalation::shadow_digest(&card);
        let stale = request.digest != digest;
        let recommendation = pm_escalation::code_recommendation(&card);
        let policy_revision = ctx.store.effective_operating_policy()?.revision;
        let project = card
            .task
            .as_deref()
            .map(|task| ctx.store.head(task).map(|head| head.project.clone()))
            .transpose()?
            .flatten();
        let decision = pm_shadow::record(&ctx.store, |shadow| {
            let repeats = shadow.repeats(&digest, envelope.identity.generation);
            let baseline = pm_escalation::escalate(
                &card,
                &EscalationContext {
                    repeats,
                    choice: recommendation,
                    requested_escalation: false,
                },
            );
            let agrees = if stale {
                None
            } else {
                recommendation.map(|r| r == request.choice)
            };
            let escalation = pm_escalation::escalate(
                &card,
                &EscalationContext {
                    repeats,
                    choice: Some(&request.choice),
                    requested_escalation: request.requested_escalation,
                },
            );
            Ok(PmShadowDecision {
                id: String::new(),
                at: corral_core::clock::now(),
                generation: envelope.identity.generation,
                project,
                task: card.task.clone(),
                kind: serde_json::to_value(card.kind)?
                    .as_str()
                    .ok_or_else(|| bad("card kind is not a string"))?
                    .to_owned(),
                // Preserve the model's presented identity, not the replacement current card.
                digest: request.digest,
                policy_revision,
                model: request.model,
                choice: request.choice,
                requested_escalation: request.requested_escalation,
                reason: request.reason,
                code_recommendation: recommendation.map(str::to_owned),
                agrees,
                stale,
                baseline_required: baseline.required,
                baseline_reasons: baseline
                    .reasons
                    .into_iter()
                    .map(|r| r.as_str().to_owned())
                    .collect(),
                escalation_required: escalation.required,
                escalation_reasons: escalation
                    .reasons
                    .into_iter()
                    .map(|r| r.as_str().to_owned())
                    .collect(),
            })
        })
        .map_err(|error| {
            if let Some(invalid) = error.downcast_ref::<pm_shadow::InvalidDecision>() {
                bad(&invalid.to_string())
            } else {
                error.context("record PM shadow decision")
            }
        })?;
        Ok(Commit {
            result: json!({
                "recorded": true, "id": decision.id, "stale": decision.stale,
                "code_recommendation": decision.code_recommendation, "agrees": decision.agrees,
                "escalation": { "required": decision.escalation_required, "reasons": decision.escalation_reasons },
            }),
            effect: None,
            post_pending: Vec::new(),
        })
    })
}

#[cfg(test)]
#[path = "pm_shadow_tests.rs"]
mod tests;
