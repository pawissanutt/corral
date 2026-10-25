//! Shared notice claims and their replayable facts, independent of classification.
use std::collections::BTreeSet;

use anyhow::ensure;

use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub const MAX_NOTICE_OUTBOX: usize = 32;

fn version_one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionLedger {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notice_outbox: Vec<NoticeClaim>,
}

impl Default for AttentionLedger {
    fn default() -> Self {
        Self {
            version: 1,
            notice_outbox: Vec::new(),
        }
    }
}

impl AttentionLedger {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Refuse backpressure before committing a new notifying transition.
    ///
    /// # Errors
    /// Returns an accounting-backpressure error at capacity or for a retained source.
    pub fn push_notice(&mut self, claim: NoticeClaim) -> Result<()> {
        ensure!(
            self.notice_outbox.len() < MAX_NOTICE_OUTBOX,
            "notification_accounting_backpressure: task notice outbox is full"
        );
        ensure!(
            !self.notice_outbox.iter().any(|existing| {
                existing.producer == claim.producer && existing.source == claim.source
            }),
            "notification_accounting_backpressure: source already has a retained notice"
        );
        self.notice_outbox.push(claim);
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.notice_outbox.len() <= MAX_NOTICE_OUTBOX,
            "notification_accounting_backpressure: task notice outbox exceeds {MAX_NOTICE_OUTBOX}"
        );
        let mut sources = BTreeSet::new();
        for claim in &self.notice_outbox {
            ensure!(
                sources.insert((&claim.producer, &claim.source)),
                "notification_accounting_backpressure: source already has a retained notice"
            );
        }
        Ok(())
    }
}

/// One physical notification batch and its complete immutable coverage relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeClaim {
    pub version: u32,
    pub batch_id: String,
    pub producer: String,
    /// Stable source episode identity, not merely a producer or task name.
    pub source: String,
    pub claimed_at: Timestamp,
    pub claim_ids: Vec<String>,
    pub coverage: Vec<NoticeCoverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exemption_basis: Option<String>,
    /// Absent before invocation AND after a call whose outcome was not persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<NoticeOutcome>,
    /// Facts remain here until their event append has been acknowledged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<NoticeFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoticeCoverage {
    Exact {
        condition_key: String,
    },
    Episode {
        task: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run: Option<String>,
        stage: String,
        at: Timestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoticeOutcome {
    Notified { at: Timestamp },
    Failed { at: Timestamp, error: String },
    NotInvoked { at: Timestamp, reason: String },
}

/// Event payload frozen at the source transition; replay never invokes transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeFact {
    /// Deterministic identity, e.g. `<batch_id>/claimed` or `<batch_id>/outcome`.
    pub fact_id: String,
    pub kind: String,
    pub detail: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn notice(source: &str) -> NoticeClaim {
        let batch_id = format!("infra_hold/{source}");
        NoticeClaim {
            version: 1,
            batch_id: batch_id.clone(),
            producer: "infra_hold".into(),
            source: source.into(),
            claimed_at: Timestamp::from_second(1).unwrap(),
            claim_ids: vec![batch_id.clone()],
            coverage: vec![NoticeCoverage::Exact {
                condition_key: format!("T-001/infra_hold/{source}"),
            }],
            exemption_basis: None,
            outcome: None,
            pending: vec![NoticeFact {
                fact_id: format!("{batch_id}/claimed"),
                kind: "notification_claimed".into(),
                detail: json!({"batch_id": batch_id}),
            }],
        }
    }

    #[test]
    fn empty_ledger_defaults_to_version_one() {
        let ledger: AttentionLedger = serde_json::from_str("{}").unwrap();
        assert_eq!(ledger, AttentionLedger::default());
    }

    #[test]
    fn notice_replay_preserves_fact_ids_coverage_and_unknown_outcome() {
        let mut claim = notice("R1");
        claim.pending.push(NoticeFact {
            fact_id: format!("{}/infra_hold", claim.batch_id),
            kind: "infra_hold".into(),
            detail: json!({"run": "R1", "class": "contention"}),
        });
        let encoded = serde_json::to_value(&claim).unwrap();
        assert!(encoded.get("outcome").is_none());
        let replayed: NoticeClaim = serde_json::from_value(encoded).unwrap();
        assert_eq!(replayed, claim);
        assert!(replayed.outcome.is_none());
        claim.outcome = Some(NoticeOutcome::Failed {
            at: Timestamp::from_second(2).unwrap(),
            error: "transport".into(),
        });
        claim.pending.push(NoticeFact {
            fact_id: format!("{}/outcome", claim.batch_id),
            kind: "owner_notify_failed".into(),
            detail: json!({"batch_id": claim.batch_id}),
        });
        let restored: NoticeClaim =
            serde_json::from_str(&serde_json::to_string(&claim).unwrap()).unwrap();
        assert_eq!(restored, claim);
    }

    #[test]
    fn notice_admission_preserves_pending_claims_at_capacity() {
        let mut ledger = AttentionLedger::default();
        ledger.push_notice(notice("R0")).unwrap();
        let previous = ledger.clone();
        let duplicate = ledger.push_notice(notice("R0")).unwrap_err();
        let error = duplicate.to_string();
        assert!(error.contains("notification_accounting_backpressure"));
        assert_eq!(ledger, previous);
        for index in 1..MAX_NOTICE_OUTBOX {
            ledger.push_notice(notice(&format!("R{index}"))).unwrap();
        }
        let previous = ledger.clone();
        assert!(ledger.push_notice(notice("overflow")).is_err());
        assert_eq!(ledger, previous);
        ledger.validate().unwrap();
        ledger.notice_outbox.push(notice("overflow"));
        assert!(ledger.validate().is_err());
    }
}
