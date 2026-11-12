//! Bounded, observational PM choices. No record here grants board authority.
use crate::{Store, records::RecordArea};
use anyhow::{Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const MAX_DECISIONS: usize = 512;
pub const MAX_DECISION_BYTES: usize = 4096;
const AREA: RecordArea<PmShadow> = RecordArea::new("pm-shadow");
const SCOPE: &str = "workspace";

#[derive(Debug)]
pub struct InvalidDecision(pub &'static str);
impl std::fmt::Display for InvalidDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for InvalidDecision {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PmShadowDecision {
    pub id: String,
    pub at: Timestamp,
    pub generation: u32,
    pub project: Option<String>,
    pub task: Option<String>,
    pub kind: String,
    pub digest: String,
    pub policy_revision: String,
    pub model: String,
    pub choice: String,
    pub requested_escalation: bool,
    pub reason: Option<String>,
    pub code_recommendation: Option<String>,
    pub agrees: Option<bool>,
    pub stale: bool,
    /// Code-only escalation, before model request/disagreement rules.
    #[serde(default)]
    pub baseline_required: bool,
    #[serde(default)]
    pub baseline_reasons: Vec<String>,
    pub escalation_required: bool,
    pub escalation_reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PmShadowCounters {
    pub total: u64,
    pub stale: u64,
    pub agree: u64,
    pub disagree: u64,
    pub escalate_requested: u64,
    pub escalation_required: u64,
    pub missed_escalation: u64,
    pub needless_escalation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PmShadow {
    pub version: u32,
    pub counters: PmShadowCounters,
    pub decisions: VecDeque<PmShadowDecision>,
}
impl Default for PmShadow {
    fn default() -> Self {
        Self {
            version: 1,
            counters: PmShadowCounters::default(),
            decisions: VecDeque::new(),
        }
    }
}
impl PmShadow {
    pub fn repeats(&self, digest: &str, generation: u32) -> usize {
        self.decisions
            .iter()
            .filter(|d| !d.stale && d.digest == digest && d.generation == generation)
            .count()
    }

    fn push(&mut self, mut decision: PmShadowDecision) -> Result<PmShadowDecision> {
        let total = self
            .counters
            .total
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("shadow counter overflow"))?;
        decision.id = format!("shadow-{total}");
        ensure!(
            decision.reason.as_ref().is_none_or(|r| r.len() <= 500),
            InvalidDecision("shadow reason exceeds 500 bytes")
        );
        ensure!(
            !decision.model.is_empty() && decision.model.len() <= 256,
            InvalidDecision("shadow model must be 1..256 bytes")
        );
        ensure!(
            serde_json::to_vec(&decision)?.len() <= MAX_DECISION_BYTES,
            InvalidDecision("shadow decision exceeds byte bound")
        );
        let counters = &mut self.counters;
        counters.total = total;
        if decision.stale {
            decision.agrees = None;
            counters.stale = counters.stale.saturating_add(1);
        } else {
            let escalated = decision.choice == "escalate" || decision.requested_escalation;
            counters.agree = counters
                .agree
                .saturating_add(u64::from(decision.agrees == Some(true)));
            counters.disagree = counters
                .disagree
                .saturating_add(u64::from(decision.agrees == Some(false)));
            counters.escalate_requested = counters
                .escalate_requested
                .saturating_add(u64::from(escalated));
            counters.escalation_required = counters
                .escalation_required
                .saturating_add(u64::from(decision.baseline_required));
            counters.missed_escalation = counters
                .missed_escalation
                .saturating_add(u64::from(decision.baseline_required && !escalated));
            counters.needless_escalation = counters
                .needless_escalation
                .saturating_add(u64::from(!decision.baseline_required && escalated));
        }
        if self.decisions.len() == MAX_DECISIONS {
            self.decisions.pop_front();
        }
        self.decisions.push_back(decision.clone());
        Ok(decision)
    }
}

pub fn load(store: &Store) -> Result<PmShadow> {
    let record = AREA.load(store, SCOPE)?.unwrap_or_default();
    ensure!(record.version == 1, "unsupported PM shadow version");
    ensure!(
        record.decisions.len() <= MAX_DECISIONS,
        "PM shadow ring exceeds bound"
    );
    Ok(record)
}

/// The builder observes the locked recurrence ring. A surrounding native receipt transaction
/// includes this write atomically; replay never invokes the builder again.
pub fn record(
    store: &Store,
    build: impl FnOnce(&PmShadow) -> Result<PmShadowDecision>,
) -> Result<PmShadowDecision> {
    let _lock = store.lock("pm-shadow")?;
    store.records_transaction(|| {
        let mut shadow = load(store)?;
        let decision = build(&shadow)?;
        let decision = shadow.push(decision)?;
        AREA.save(store, SCOPE, &shadow)?;
        Ok(decision)
    })
}

#[cfg(test)]
#[path = "pm_shadow_tests.rs"]
mod tests;
