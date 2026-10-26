//! Shared notice claims and their replayable facts, independent of classification.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::ensure;

use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Stable identity of one canonical condition, independent of display precedence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConditionKey(pub String);

impl std::fmt::Display for ConditionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Consumers render labels rather than exhaustively matching this extensible vocabulary.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitClass {
    OwnerDecision,
    PmAction,
    ExternalOperation,
    Prerequisite,
    Capacity,
    Hold,
    Parked,
    NotScheduled,
}

impl WaitClass {
    pub const fn label(self) -> &'static str {
        match self {
            Self::OwnerDecision => "Owner decision",
            Self::PmAction => "PM action",
            Self::ExternalOperation => "External operation",
            Self::Prerequisite => "Prerequisite",
            Self::Capacity => "Capacity",
            Self::Hold => "Hold",
            Self::Parked => "Parked",
            Self::NotScheduled => "Not scheduled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Responsible {
    Pm,
    Owner,
    Executor,
    External,
}

impl Responsible {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pm => "PM",
            Self::Owner => "Owner",
            Self::Executor => "Executor",
            Self::External => "External",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinceSource {
    Canonical,
    Discovered,
}

impl SinceSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Discovered => "discovered",
        }
    }
}

/// A check description is data, never executable text or an authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCheck {
    pub kind: String,
    pub target: Option<String>,
    pub expected: Option<String>,
    pub description: String,
    pub machine_checkable: bool,
    /// None means the required external evidence is not available to this projection.
    pub satisfied: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextAction {
    pub command: Option<String>,
    pub description: String,
    pub requires_owner: bool,
    pub recipient: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    /// Capacity, prerequisite and policy waits deliberately have no ladder identity.
    pub key: Option<ConditionKey>,
    pub kind: String,
    pub class: WaitClass,
    pub reason_code: String,
    pub reason: String,
    pub responsible: Responsible,
    pub since: Option<Timestamp>,
    pub since_source: SinceSource,
    pub age_secs: Option<i64>,
    pub resume: Vec<ResumeCheck>,
    pub next_action: NextAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub executor: String,
    pub since: Option<Timestamp>,
    pub reason: String,
    pub liveness: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    pub conditions: Vec<Condition>,
    pub progress: Option<Progress>,
    /// A copy of a condition, not a second identity or a resolution rule.
    pub primary: Option<Condition>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionLevels {
    pub announced_at: Option<Timestamp>,
    pub rewoke_at: Option<Timestamp>,
    pub owner_claimed_at: Option<Timestamp>,
    pub owner_claim: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionHandling {
    pub revision: u32,
    pub request: String,
    pub note: String,
    pub at: Timestamp,
    pub next_check: Timestamp,
    pub by: Responsible,
}

/// Durable first-discovery time is used only when the canonical source has no timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionRecord {
    pub kind: String,
    pub responsible: Responsible,
    pub since: Timestamp,
    pub since_source: SinceSource,
    pub first_recorded: Timestamp,
    pub opened_at: Timestamp,
    pub pm_live_at_open: Option<bool>,
    #[serde(default)]
    pub levels: AttentionLevels,
    pub handling: Option<AttentionHandling>,
    pub first_handled_at: Option<Timestamp>,
    pub resolved_at: Option<Timestamp>,
}

pub const MAX_NOTICE_OUTBOX: usize = 32;
pub const MAX_ATTENTION_RECORDS: usize = 32;
pub const MAX_ATTENTION_NOTE_BYTES: usize = 1024;
pub const MAX_ATTENTION_LEDGER_BYTES: usize = 2 * 1024 * 1024;

/// A selector for one Needs me status episode, not an acknowledgement or authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryNotice {
    pub batch_id: String,
    pub at: Timestamp,
    pub status_since: Option<Timestamp>,
}

/// A keyed pointer to an existing standalone producer record. It is not a send claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceNoticeRef {
    pub area: String,
    pub key: String,
    pub claim_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionFact {
    pub key: ConditionKey,
    pub fact_id: String,
    pub kind: String,
    pub detail: serde_json::Value,
}

fn version_one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionLedger {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub records: BTreeMap<ConditionKey, AttentionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_notice: Option<EntryNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbox: Vec<AttentionFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notice_outbox: Vec<NoticeClaim>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_notices: Vec<SourceNoticeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow_count: Option<usize>,
}

impl Default for AttentionLedger {
    fn default() -> Self {
        Self {
            version: 1,
            records: BTreeMap::new(),
            entry_notice: None,
            outbox: Vec::new(),
            notice_outbox: Vec::new(),
            source_notices: Vec::new(),
            overflow_count: None,
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
            self.notice_outbox.len() + self.source_notices.len() < MAX_NOTICE_OUTBOX,
            "notification_accounting_backpressure: task notice outbox is full"
        );
        ensure!(
            !self.notice_outbox.iter().any(|existing| {
                existing.producer == claim.producer && existing.source == claim.source
            }),
            "notification_accounting_backpressure: source already has a retained notice"
        );
        let previous = self.notice_outbox.len();
        self.notice_outbox.push(claim);
        if let Err(error) = self.validate() {
            self.notice_outbox.truncate(previous);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.records.len() <= 32,
            "attention ledger exceeds 32 records"
        );
        ensure!(
            self.notice_outbox.len() + self.source_notices.len() <= MAX_NOTICE_OUTBOX,
            "notification_accounting_backpressure: task notice outbox exceeds {MAX_NOTICE_OUTBOX}"
        );
        let mut sources = BTreeSet::new();
        for claim in &self.notice_outbox {
            ensure!(
                sources.insert((&claim.producer, &claim.source)),
                "notification_accounting_backpressure: source already has a retained notice"
            );
        }
        ensure!(
            self.source_notices.len() <= 32,
            "notification_accounting_backpressure: source references full"
        );
        let mut pending = BTreeMap::<&ConditionKey, usize>::new();
        for fact in &self.outbox {
            ensure!(
                self.records.contains_key(&fact.key),
                "attention fact has no retained record"
            );
            let count = pending.entry(&fact.key).or_default();
            *count += 1;
            ensure!(
                *count <= 4,
                "attention_accounting_backpressure: condition outbox full"
            );
        }
        for (key, record) in &self.records {
            ensure!(
                key.0.len() <= 1024 && record.kind.len() <= 64,
                "attention identity exceeds bound"
            );
            if let Some(handling) = &record.handling {
                ensure!(
                    handling.note.len() <= MAX_ATTENTION_NOTE_BYTES
                        && handling.request.len() <= 128,
                    "attention handling exceeds bound"
                );
            }
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_ATTENTION_LEDGER_BYTES,
            "attention_accounting_backpressure: serialized ledger exceeds byte bound"
        );
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
