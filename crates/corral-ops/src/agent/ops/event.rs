//! event: host lifecycle facts (turn start and end, provider errors) recorded on the container
//! with their receipt. Classification of an ended turn belongs to the role children.
use anyhow::Result;
use corral_core::agent::{
    AgentTurn, ClosureReason, GenerationFence, HostHealth, ReceiptEffect, TURNS_MAX, TurnClass,
    TurnOrigin, TurnOriginKind,
};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::{Envelope, event_id_parts};
use crate::agent::receipts::{self, Commit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EventKind {
    TurnStarted,
    TurnEnded,
    ProviderError,
    BudgetExhausted,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventPayload {
    kind: EventKind,
    turn: u64,
    #[serde(default)]
    origin: Option<TurnOrigin>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    budget: Option<serde_json::Value>,
    /// turn_ended only: the owner typed into the pane while this turn was open.
    #[serde(default)]
    owner_input: bool,
    /// turn_ended only: the host cancelled this turn on `corral agent cancel` (recovery), so it
    /// classifies `cancelled` in the transaction that ends it, whenever corral's own mark lands.
    #[serde(default)]
    cancelled: bool,
}

fn bad(reason: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn word(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

fn validate(envelope: &Envelope, payload: &EventPayload) -> Result<()> {
    if event_id_parts(&envelope.request_id).is_none() {
        return Err(bad("lifecycle events use request_id ev-<generation>-<seq>"));
    }
    if envelope
        .identity
        .turn
        .is_some_and(|turn| turn != payload.turn)
    {
        return Err(bad("identity.turn differs from payload.turn"));
    }
    if payload.origin.is_some() && payload.kind != EventKind::TurnStarted {
        return Err(bad("only turn_started carries an origin"));
    }
    if payload.provider.is_some() != payload.model.is_some()
        || ((payload.provider.is_some() || payload.model.is_some())
            && payload.kind != EventKind::TurnStarted)
        || payload.provider.as_ref().is_some_and(|p| !word(p, 64))
        || payload
            .model
            .as_ref()
            .is_some_and(|m| m.is_empty() || m.len() > 1024 || m.chars().any(char::is_control))
    {
        return Err(bad(
            "only turn_started carries a provider/model pair of bounded identifiers",
        ));
    }
    if payload.owner_input && payload.kind != EventKind::TurnEnded {
        return Err(bad("only turn_ended carries owner_input"));
    }
    if payload.budget.is_some() && payload.kind != EventKind::BudgetExhausted {
        return Err(bad("only budget_exhausted carries budget state"));
    }
    if payload.cancelled && payload.kind != EventKind::TurnEnded {
        return Err(bad("only turn_ended carries cancelled"));
    }
    let key = payload
        .origin
        .as_ref()
        .and_then(|o| o.idempotency_key.as_deref());
    if key.is_some_and(|key| !word(key, 128)) {
        return Err(bad(
            "origin.idempotency_key must be 1-128 characters of [A-Za-z0-9._:-]",
        ));
    }
    match (&payload.class, payload.kind) {
        (Some(class), EventKind::ProviderError) if word(class, 64) => Ok(()),
        (None, EventKind::TurnStarted | EventKind::TurnEnded | EventKind::BudgetExhausted) => {
            Ok(())
        }
        _ => Err(bad(
            "provider_error needs a class of [A-Za-z0-9._:-]; other events carry none",
        )),
    }
}

fn evict(turns: &mut Vec<AgentTurn>) {
    while turns.len() > TURNS_MAX {
        let Some(oldest) = turns.iter().position(|t| t.ended.is_some()) else {
            return;
        };
        turns.remove(oldest);
    }
}

fn turn(generation: u32, seq: u64) -> AgentTurn {
    AgentTurn {
        generation,
        seq,
        origin: TurnOrigin {
            kind: TurnOriginKind::Unknown,
            idempotency_key: None,
        },
        started: None,
        ended: None,
        class: None,
        provider_error: None,
        provider: None,
        model: None,
    }
}

pub(crate) fn event(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    if envelope.payload.get("kind").and_then(|v| v.as_str()) == Some("budget") {
        return super::budget::budget(ctx, envelope);
    }
    if envelope.payload.get("kind").and_then(|v| v.as_str()) == Some("auxiliary") {
        return super::auxiliary::auxiliary(ctx, envelope);
    }
    if envelope.payload.get("kind").and_then(|v| v.as_str()) == Some("owner_input") {
        return super::owner_input::journal(ctx, envelope);
    }
    let payload: EventPayload = super::payload(
        envelope,
        "kind, turn, origin?, class?, owner_input?, cancelled?",
    )?;
    validate(envelope, &payload)?;
    let container = admission::container_of(&envelope.identity);
    let settles_turn = (payload.kind == EventKind::TurnEnded).then_some(payload.turn);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let n = envelope.identity.generation;
    let kind_name = payload.kind_name();
    let EventPayload {
        kind,
        turn: seq,
        origin,
        provider,
        model,
        class,
        budget,
        owner_input,
        cancelled,
    } = payload;
    receipts::commit(ctx, &admitted, move |state, closure| {
        let now = Timestamp::now();
        let index = state
            .turns
            .iter()
            .position(|t| t.generation == n && t.seq == seq);
        let mut post_pending = Vec::new();
        match (kind, index) {
            (EventKind::TurnStarted, Some(_)) => return Err(bad("the turn already started")),
            (EventKind::TurnStarted, None) => {
                let mut started = turn(n, seq);
                started.started = Some(now);
                started.provider = provider.clone();
                started.model = model.clone();
                if let Some(origin) = origin {
                    started.origin = origin;
                }
                state.turns.push(started);
            }
            (EventKind::TurnEnded, Some(i))
                if state.turns[i].started.is_some() && state.turns[i].ended.is_none() =>
            {
                state.turns[i].ended = Some(now);
                if owner_input {
                    state.turns[i].origin.kind = TurnOriginKind::OwnerTerminal;
                }
                if cancelled && state.turns[i].class != Some(TurnClass::BudgetExhausted) {
                    state.turns[i].class = Some(TurnClass::Cancelled);
                }
                crate::agent::turns::classify_ended(state, closure, i);
                post_pending.push("event:agent_turn_ended".into());
            }
            (EventKind::TurnEnded, _) => {
                return Err(bad(
                    "turn_ended must name a started turn that is still open",
                ));
            }
            (EventKind::BudgetExhausted, index) => {
                let i = index.unwrap_or_else(|| {
                    state.turns.push(turn(n, seq));
                    state.turns.len() - 1
                });
                state.turns[i].class = Some(TurnClass::BudgetExhausted);
                post_pending.push("event:agent_budget_exhausted".into());
            }
            (EventKind::ProviderError, index) => {
                let i = index.unwrap_or_else(|| {
                    state.turns.push(turn(n, seq));
                    state.turns.len() - 1
                });
                state.turns[i].provider_error = class.clone();
                post_pending.push("event:agent_provider_error".into());
            }
        }
        evict(&mut state.turns);
        let closes = settles_turn.is_some()
            && (state.binding.closing || closure == Some(ClosureReason::ReviewClosed));
        if closes && let Some(generation) = state.generation_mut(n) {
            generation.fenced.get_or_insert(GenerationFence {
                reason: "review_closed".into(),
                at: now,
            });
            state.binding.health = HostHealth::Fenced;
            post_pending.push("event:agent_fenced".into());
        }
        Ok(Commit {
            result: json!({
                "kind": kind_name, "turn": seq, "generation": n,
                "class": if kind == EventKind::BudgetExhausted { Some("budget_exhausted") } else { class.as_deref() }, "fenced": closes,
                "budget": budget, "provider": provider, "model": model,
            }),
            effect: Some(ReceiptEffect {
                kind: "turn".into(),
                reference: format!("{n}/{seq}"),
            }),
            post_pending,
        })
    })
}

impl EventPayload {
    fn kind_name(&self) -> &'static str {
        match self.kind {
            EventKind::TurnStarted => "turn_started",
            EventKind::TurnEnded => "turn_ended",
            EventKind::ProviderError => "provider_error",
            EventKind::BudgetExhausted => "budget_exhausted",
        }
    }
}
