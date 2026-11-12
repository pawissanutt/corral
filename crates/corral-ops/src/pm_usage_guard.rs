//! PM-only request cache-health guard. Cost observations are never spending limits.
use std::collections::VecDeque;

use anyhow::{Context, Result};
use corral_core::{agent::pm_state, config::PmGuardCfg, records::RecordArea};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Ctx;

const RECORD: RecordArea<Guard> = RecordArea::new("pm-usage-guard");
const KEY: &str = "current";
const HISTORY: usize = 8;

/// Pi's provider-normalized usage uses disjoint uncached, cache-read and cache-write
/// input buckets (the same fields as agent/usage.rs's WireUsage).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RequestUsage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl RequestUsage {
    pub(crate) fn total_input(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    fn ratio(&self) -> f64 {
        let total = self.total_input();
        if total == 0 {
            0.0
        } else {
            self.cache_read as f64 / total as f64
        }
    }

    pub(crate) fn valid(&self, tokens: u64) -> bool {
        [self.input, self.cache_read, self.cache_write, self.output]
            .into_iter()
            .try_fold(0_u64, u64::checked_add)
            == Some(tokens)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Observation {
    pub generation: u32,
    pub request: String,
    pub response: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub at: Timestamp,
    pub usage: RequestUsage,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PauseReason {
    CacheGuardPaused,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum State {
    #[default]
    Ok,
    Paused {
        since: Timestamp,
        reason: PauseReason,
    },
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Guard {
    generation: u32,
    observations: VecDeque<Observation>,
    consecutive_misses: u32,
    state: State,
}

impl Guard {
    fn for_generation(&mut self, generation: u32) {
        if self.generation != generation {
            *self = Self {
                generation,
                ..Self::default()
            };
        }
    }

    fn paused(&self) -> bool {
        matches!(self.state, State::Paused { .. })
    }

    /// Only transitions emit reports: report-only mode also reports once per miss episode.
    fn observe(
        &mut self,
        cfg: &PmGuardCfg,
        observation: Observation,
    ) -> Vec<(&'static str, Value)> {
        self.for_generation(observation.generation);
        let total = observation.usage.total_input();
        let ratio = observation.usage.ratio();
        let mut events = Vec::new();
        let detail = || {
            json!({
                "generation":observation.generation, "request":observation.request,
                "response":observation.response, "model":observation.model,
                "provider":observation.provider, "usage":observation.usage,
                "total_input_tokens":total, "cache_read_ratio":ratio,
            })
        };
        if let Some(previous) = self.observations.back()
            && previous.usage.total_input() < cfg.price_tier_input_tokens
            && total >= cfg.price_tier_input_tokens
        {
            let mut event = detail();
            event["previous_input_tokens"] = json!(previous.usage.total_input());
            event["price_tier_input_tokens"] = json!(cfg.price_tier_input_tokens);
            events.push(("pm_price_tier_crossed", event));
        }
        let previous_misses = self.consecutive_misses;
        if total >= cfg.large_context_tokens && ratio < cfg.min_cache_read_ratio {
            self.consecutive_misses = self.consecutive_misses.saturating_add(1);
        } else {
            self.consecutive_misses = 0;
        }
        if previous_misses < cfg.consecutive_misses
            && self.consecutive_misses >= cfg.consecutive_misses
            && !self.paused()
        {
            if cfg.pause {
                self.state = State::Paused {
                    since: observation.at,
                    reason: PauseReason::CacheGuardPaused,
                };
            }
            let mut event = detail();
            event["consecutive_misses"] = json!(self.consecutive_misses);
            event["large_context_tokens"] = json!(cfg.large_context_tokens);
            event["min_cache_read_ratio"] = json!(cfg.min_cache_read_ratio);
            event["paused"] = json!(cfg.pause);
            events.push(("pm_cache_miss_guard", event));
        }
        if self.observations.len() >= HISTORY {
            self.observations.pop_front();
        }
        self.observations.push_back(observation);
        events
    }
}

/// Called only inside the PM budget receipt's dispatch-serialized records transaction.
/// The record and its events therefore commit (or roll back) with the settle receipt.
pub(crate) fn observe(ctx: &Ctx, observation: Observation) -> Result<bool> {
    let mut guard = RECORD.load(&ctx.store, KEY)?.unwrap_or_default();
    for (kind, detail) in guard.observe(&ctx.store.config.pm.guard, observation) {
        ctx.store
            .log_event(None, kind, detail)
            .with_context(|| format!("recording {kind}"))?;
    }
    RECORD.save(&ctx.store, KEY, &guard)?;
    Ok(guard.paused())
}

/// Called inside a PM reserve/state transaction. A generation boundary discards the
/// old cache history before admission; unlike a cost budget, it is not cumulative.
pub(crate) fn paused(ctx: &Ctx, generation: u32) -> Result<bool> {
    let Some(mut guard) = RECORD.load(&ctx.store, KEY)? else {
        return Ok(false);
    };
    if guard.generation != generation {
        guard.for_generation(generation);
        RECORD.save(&ctx.store, KEY, &guard)?;
    }
    Ok(guard.paused())
}

/// Owner command only: clear the cache hold without killing or replacing the session.
/// The CLI, not model tools, exposes this operation; the next normal reserve rechecks it.
pub fn resume(ctx: &Ctx) -> Result<bool> {
    let _dispatch = ctx
        .store
        .try_lock("dispatch")?
        .context("PM admission is busy; retry guard-resume")?;
    ctx.store.records_transaction(|| {
        let Some(mut guard) = RECORD.load(&ctx.store, KEY)? else {
            return Ok(false);
        };
        let generation = pm_state::load_current(&ctx.store)?
            .map(|pm| pm.state.binding.generation)
            .unwrap_or(guard.generation);
        guard.for_generation(generation);
        let was_paused = guard.paused();
        guard.state = State::Ok;
        guard.consecutive_misses = 0;
        RECORD.save(&ctx.store, KEY, &guard)?;
        Ok(was_paused)
    })
}

/// Read-only current-generation projection, including unwatched stores.
pub(crate) fn health(ctx: &Ctx) -> Result<Value> {
    let mut guard = RECORD.load(&ctx.store, KEY)?.unwrap_or_default();
    if let Some(pm) = pm_state::load_current(&ctx.store)? {
        guard.for_generation(pm.state.binding.generation);
    }
    Ok(json!({
        "generation":guard.generation, "status":guard.state,
        "consecutive_misses":guard.consecutive_misses,
        "last_ratio":guard.observations.back().map(|o| o.usage.ratio()),
        "last_total_input_tokens":guard.observations.back().map(|o| o.usage.total_input()),
    }))
}

#[cfg(test)]
#[path = "pm_usage_guard_tests.rs"]
mod tests;
