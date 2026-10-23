//! The asking agent's operations: ask opens a question and withdraw retracts it. Both run inside
//! the commit transaction of the asking container (`agent::call`), next to their receipt.
use anyhow::Result;
use corral_core::agent::{
    AGENT_RECORD_VERSION, AgentQuestion, AgentRunState, BLOCKING_QUESTIONS_MAX, ContainerRef,
    HostHealth, OutboxState, PENDING_QUESTIONS_MAX, QUESTION_CONTEXT_REFS_MAX,
    QUESTION_OPTIONS_MAX, QUESTION_TEXT_MAX_BYTES, QuestionActor, QuestionAsked, QuestionKind,
    QuestionState, QuestionWake, QuestionWakeRoute, RequestOrigin, pm_state,
};
use jiff::Timestamp;
use serde::Deserialize;

use super::{QuestionKey, QuestionRefusal, RefusalKind, bounded, question_key, transition};
use crate::Ctx;

pub const OPTION_MAX_BYTES: usize = 512;
pub const REF_MAX_BYTES: usize = 256;
pub const NOTE_MAX_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskPayload {
    pub kind: QuestionKind,
    #[serde(default)]
    pub blocking: bool,
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub recommendation: Option<String>,
    #[serde(default)]
    pub context_refs: Vec<String>,
    #[serde(default)]
    pub needed_by: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawPayload {
    pub key: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub key: String,
    pub revision: u32,
    pub blocking: bool,
}

pub fn validate_ask(payload: &AskPayload) -> Result<(), QuestionRefusal> {
    bounded(&payload.text, QUESTION_TEXT_MAX_BYTES, "text")?;
    let count = |what: &str, len: usize, max: usize| {
        if len > max {
            Err(QuestionRefusal::new(
                RefusalKind::Bounds,
                format!("{len} {what}; the bound is {max}"),
            ))
        } else {
            Ok(())
        }
    };
    count("options", payload.options.len(), QUESTION_OPTIONS_MAX)?;
    count(
        "context_refs",
        payload.context_refs.len(),
        QUESTION_CONTEXT_REFS_MAX,
    )?;
    for option in &payload.options {
        bounded(option, OPTION_MAX_BYTES, "option")?;
    }
    for reference in &payload.context_refs {
        bounded(reference, REF_MAX_BYTES, "context ref")?;
    }
    if let Some(recommendation) = &payload.recommendation {
        bounded(recommendation, QUESTION_TEXT_MAX_BYTES, "recommendation")?;
    }
    Ok(())
}

/// A bound, healthy Pi-native PM receives a question wake through its outbox; an OmO PM gets the
/// existing push only when the owner enabled `question_opened`; otherwise there is no route.
pub fn wake_route(ctx: &Ctx) -> Result<QuestionWakeRoute> {
    let pm_live = pm_state::load(&ctx.store)?.is_some_and(|pm| {
        pm.state.binding.health == HostHealth::Healthy
            && pm
                .state
                .current_generation()
                .is_some_and(|g| g.fenced.is_none())
    });
    let config = &ctx.store.config.pm;
    Ok(if pm_live {
        QuestionWakeRoute::PmInbox
    } else if config.push_events && config.push_kinds.iter().any(|k| k == "question_opened") {
        QuestionWakeRoute::OmoPush
    } else {
        QuestionWakeRoute::None
    })
}

fn limit(reason: String) -> QuestionRefusal {
    QuestionRefusal::new(RefusalKind::Limit, reason)
}

/// Opens Q<n> with revision 1. Counts use recorded state: the commit's liveness check already
/// proved the container open.
pub fn open(
    state: &mut AgentRunState,
    container: &ContainerRef,
    origin: &RequestOrigin,
    payload: AskPayload,
    route: QuestionWakeRoute,
    now: Timestamp,
) -> Result<Opened, QuestionRefusal> {
    validate_ask(&payload)?;
    let pending: Vec<&AgentQuestion> = state
        .questions
        .iter()
        .filter(|q| q.recorded_pending())
        .collect();
    if pending.len() >= PENDING_QUESTIONS_MAX {
        return Err(limit(format!(
            "{} already has {PENDING_QUESTIONS_MAX} pending questions",
            container.key()
        )));
    }
    if payload.blocking && pending.iter().filter(|q| q.blocking).count() >= BLOCKING_QUESTIONS_MAX {
        return Err(limit(format!(
            "{} already has a pending blocking question",
            container.key()
        )));
    }
    let key = question_key(container, state.questions.len() + 1);
    let identity = &origin.identity;
    state.questions.push(AgentQuestion {
        version: AGENT_RECORD_VERSION,
        key: key.clone(),
        revision: 1,
        asked: QuestionAsked {
            request_id: origin.request_id.clone(),
            payload_sha256: origin.payload_sha256.clone(),
            session_id: identity.session_id.clone(),
            generation: identity.generation,
            turn: identity.turn,
            at: now,
        },
        kind: payload.kind,
        blocking: payload.blocking,
        text: payload.text,
        options: payload.options,
        recommendation: payload.recommendation,
        context_refs: payload.context_refs,
        needed_by: payload.needed_by,
        state: QuestionState::Open,
        triage: Vec::new(),
        answer: None,
        deadline: None,
        wake: QuestionWake {
            route,
            key: format!("qwake:{key}:1"),
            state: if route == QuestionWakeRoute::PmInbox {
                OutboxState::Pending
            } else {
                OutboxState::Posted
            },
            message_id: None,
        },
        unattended_at: None,
        terminal: None,
    });
    Ok(Opened {
        key,
        revision: 1,
        blocking: payload.blocking,
    })
}

/// Retracts a pending question of the caller's own container and records the reason.
pub fn withdraw(
    state: &mut AgentRunState,
    container: &ContainerRef,
    payload: &WithdrawPayload,
    now: Timestamp,
) -> Result<(String, u32), QuestionRefusal> {
    bounded(&payload.reason, NOTE_MAX_BYTES, "reason")?;
    let parsed = QuestionKey::parse(&payload.key)?;
    let not_found = || {
        QuestionRefusal::new(
            RefusalKind::NotFound,
            format!("{} is not a question of {}", payload.key, container.key()),
        )
    };
    if parsed.container != *container {
        return Err(not_found());
    }
    let question = super::find_mut(state, &payload.key).ok_or_else(not_found)?;
    match question.state {
        QuestionState::Withdrawn => {
            return Err(QuestionRefusal::new(
                RefusalKind::Withdrawn,
                format!("{} was already withdrawn", question.key),
            ));
        }
        state if !state.pending() || question.terminal.is_some() => {
            return Err(QuestionRefusal::new(
                RefusalKind::State,
                format!("{} is {state:?}, not pending", question.key),
            ));
        }
        _ => {}
    }
    transition(
        question,
        QuestionState::Withdrawn,
        "withdraw",
        QuestionActor::Agent,
        Some(payload.reason.clone()),
        now,
    );
    Ok((question.key.clone(), question.revision))
}
