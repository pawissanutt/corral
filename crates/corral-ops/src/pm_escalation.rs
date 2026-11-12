//! Code-only escalation policy; never permission to execute a card.
//!
//! Routine allow-list (live `pm_next`/workset producers still project owner permissions):
//!
//! | Kind | Placement and justification |
//! |---|---|
//! | Dispatch | Routine slot/dependency admission; owner-required options still escalate. |
//! | Gate | Starts the configured gate, never interprets or waives its result. |
//! | Review | Starts independent review, never accepts its verdict or lands. |
//! | Wait | Plain observation/resumption wait, with no board effect. |
//! | ReviewInFlight | Not validated: a review/landing barrier, not a plain wait. |
//! | Owner, Question, AgentQuestion, OwnerQuestion | Owner/question decisions always escalate. |
//! | LessonsDistill, GateLog, StaleVerdict, EvidenceOnly, LoadFailure, Diagnosis | Repair or interpretation is not validated routine work. |
//! | IdeaPromotion, Land, Inbox | Scoping, acceptance, or free-form coordination is not validated routine work. |
//!
//! The digest is SHA-256 of compact JSON `[kind, task, ordered_option_ids, recommendation]`.
//! Recommendation is null unless it names a current option. Prose and clocks are excluded.
use crate::pm_next::{PmAction, PmActionKind};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    OwnerOnly,
    NoRecommendation,
    UnvalidatedClass,
    Repetition,
    Requested,
    Disagreement,
}
impl EscalationReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnerOnly => "owner_only",
            Self::NoRecommendation => "no_recommendation",
            Self::UnvalidatedClass => "unvalidated_class",
            Self::Repetition => "repetition",
            Self::Requested => "requested",
            Self::Disagreement => "disagreement",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Escalation {
    pub required: bool,
    pub reasons: Vec<EscalationReason>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EscalationContext<'a> {
    pub repeats: usize,
    pub choice: Option<&'a str>,
    pub requested_escalation: bool,
}

pub fn code_recommendation(card: &PmAction) -> Option<&str> {
    card.options
        .iter()
        .find(|o| o.id == card.recommended)
        .map(|o| o.id.as_str())
}

pub fn shadow_digest(card: &PmAction) -> String {
    let identity = serde_json::json!([
        card.kind,
        card.task,
        card.options
            .iter()
            .map(|o| o.id.as_str())
            .collect::<Vec<_>>(),
        code_recommendation(card),
    ]);
    format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
}

pub fn escalate(card: &PmAction, ctx: &EscalationContext<'_>) -> Escalation {
    use EscalationReason::*;
    let mut reasons = Vec::new();
    if card.requires_owner
        || matches!(
            card.kind,
            PmActionKind::Owner
                | PmActionKind::Question
                | PmActionKind::AgentQuestion
                | PmActionKind::OwnerQuestion
        )
        || card.options.iter().any(|o| {
            o.requires_owner && (Some(o.id.as_str()) == ctx.choice || o.id == card.recommended)
        })
    {
        reasons.push(OwnerOnly);
    }
    let recommendation = code_recommendation(card);
    if recommendation.is_none() {
        reasons.push(NoRecommendation);
    }
    if !matches!(
        card.kind,
        PmActionKind::Dispatch | PmActionKind::Gate | PmActionKind::Review | PmActionKind::Wait
    ) {
        reasons.push(UnvalidatedClass);
    }
    if ctx.repeats >= 3 {
        reasons.push(Repetition);
    }
    if ctx.requested_escalation || ctx.choice == Some("escalate") {
        reasons.push(Requested);
    }
    if let (Some(choice), Some(recommended)) = (ctx.choice, recommendation)
        && choice != recommended
    {
        reasons.push(Disagreement);
    }
    Escalation {
        required: !reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
#[path = "pm_escalation_tests.rs"]
mod tests;
