//! Host settlement and structurally attributed accounting, independent of transport chunks.
use std::collections::BTreeMap;

use anyhow::Result;
use corral_core::assistant::threads::Message;
use corral_protocol::assistant::TurnCompletion;
use serde::{Deserialize, Serialize};

use crate::Ctx;

const COMPLETIONS: &str = "app-turn-completions";

#[derive(Serialize, Deserialize)]
struct Completion {
    generation: Option<u32>,
    turn: Option<u64>,
    outcome: TurnCompletion,
}

pub(super) fn settle(ctx: &Ctx, message: &Message, outcome: TurnCompletion) -> Result<()> {
    ctx.store.save_record(
        COMPLETIONS,
        &message.id,
        &Completion {
            generation: message.generation,
            turn: message.turn,
            outcome,
        },
    )
}

pub(super) fn completion(ctx: &Ctx, message: &Message) -> Result<Option<TurnCompletion>> {
    if message.state != "done" {
        return Ok(None);
    }
    // Legacy records lack host settlement evidence; neither a failure's wording nor a Stop
    // acknowledgement proves that the provider actually aborted.
    Ok(ctx
        .store
        .record::<Completion>(COMPLETIONS, &message.id)?
        .and_then(|record| {
            (record.generation == message.generation && record.turn == message.turn)
                .then_some(record.outcome)
        }))
}

pub(super) type Costs = BTreeMap<(u32, u64), corral_core::Usage>;

pub(super) fn costs(ctx: &Ctx, state: &corral_core::agent::AgentRunState) -> Costs {
    let mut turns: BTreeMap<_, Vec<_>> = BTreeMap::new();
    // The same collector supplies canonical session accounting and deduplicates provider
    // responses. No timestamp proximity or division of a session total attributes an ask.
    // Incomplete/unreadable evidence remains unknown, not a partial ask total.
    if let Ok(entries) = crate::agent::usage::collect_entries(state, None, &ctx.store, true) {
        for entry in entries {
            if let Some(span) = entry.span {
                turns.entry(span.turn).or_default().push(entry.usage);
            }
        }
    }
    turns
        .into_iter()
        .map(|(turn, entries)| (turn, corral_core::Usage::from_entries(entries)))
        .collect()
}

pub(super) fn cost(
    costs: &Costs,
    message: &Message,
) -> (Option<f64>, Option<corral_protocol::board::CostBasis>) {
    if message.state != "done" {
        return (None, None);
    }
    let Some(usage) = message
        .generation
        .zip(message.turn)
        .and_then(|key| costs.get(&key))
    else {
        return (None, None);
    };
    // Reported cost takes precedence. An estimate is a complete API-equivalent amount only
    // when every response is priced; mixed or missing evidence must not understate an ask.
    if let Some(cost) = usage.reported_usd {
        (
            Some(cost),
            Some(corral_protocol::board::CostBasis::Reported),
        )
    } else if usage.unpriced_tokens == 0
        && let Some(cost) = usage.estimated_usd
    {
        (
            Some(cost),
            Some(corral_protocol::board::CostBasis::Estimated),
        )
    } else {
        (None, None)
    }
}
