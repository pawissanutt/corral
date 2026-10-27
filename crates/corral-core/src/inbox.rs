//! Harness-independent inbox records. No production caller routes delivery here yet.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod envelope;
mod header;
pub use header::{
    AttemptRef, DeliveryHeader, DeliveryState, IndexPhase, LegacyDebt, OwnerEntry, OwnerHeader,
    OwnerMessage, RetryCoverage, RetryEvidence, RetryHistory, RetryRoute, RetryWitness,
    WitnessReason,
};
mod owner;
pub use owner::{Recovery, RetryInspection};
mod storage;
pub use storage::Inbox;

#[cfg(test)]
mod review_run_key_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_claim;
#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_retry;

pub const MAX_PAYLOAD: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    #[default]
    Legacy,
    Inbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Recipient {
    Run {
        task: String,
        run: String,
    },
    PlanRun {
        task: String,
        plan_run: String,
    },
    PlanReview {
        task: String,
        plan: String,
        review: String,
    },
    /// A Pi-native review child (T-252), keyed `<task>_<run>_rv-<review>`.
    ReviewRun {
        task: String,
        run: String,
        review: String,
    },
    Pm,
}

impl Recipient {
    /// Access state inside a caller's existing task transaction, without taking another lock.
    pub fn state_mut<'a>(&self, task: &'a mut crate::Task) -> Result<&'a mut InboxState> {
        storage::state_mut(task, self)
    }
    pub fn key(&self) -> Result<String> {
        let parts: Vec<&str> = match self {
            Self::Run { task, run } => vec![task, run],
            Self::PlanRun { task, plan_run } => vec![task, plan_run],
            Self::PlanReview { task, plan, review } => vec![task, plan, review],
            Self::ReviewRun { task, run, review } => vec![task, run, review],
            Self::Pm => Vec::new(),
        };
        for part in parts {
            ensure!(
                !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "invalid inbox identity component {part:?}"
            );
        }
        Ok(match self {
            Self::Run { task, run } => format!("{task}_{run}"),
            Self::PlanRun { task, plan_run } => format!("{task}_plan-{plan_run}"),
            Self::PlanReview { task, plan, review } => {
                ensure!(
                    !review.starts_with("rv-"),
                    "plan review id {review:?} collides with a review-run key"
                );
                format!("{task}_{plan}_{review}")
            }
            Self::ReviewRun { task, run, review } => format!("{task}_{run}_rv-{review}"),
            Self::Pm => "pm".into(),
        })
    }

    pub fn parse_key(key: &str) -> Result<Self> {
        let parts: Vec<_> = key.split('_').collect();
        let recipient = match parts.as_slice() {
            ["pm"] => Self::Pm,
            [task, planner] if planner.starts_with("plan-") => Self::PlanRun {
                task: (*task).into(),
                plan_run: planner["plan-".len()..].into(),
            },
            [task, run] => Self::Run {
                task: (*task).into(),
                run: (*run).into(),
            },
            [task, run, review] if review.starts_with("rv-") => Self::ReviewRun {
                task: (*task).into(),
                run: (*run).into(),
                review: review["rv-".len()..].into(),
            },
            [task, plan, review] => Self::PlanReview {
                task: (*task).into(),
                plan: (*plan).into(),
                review: (*review).into(),
            },
            _ => anyhow::bail!("invalid inbox key {key:?}"),
        };
        ensure!(recipient.key()? == key, "noncanonical inbox key");
        Ok(recipient)
    }

    pub fn task(&self) -> Option<&str> {
        match self {
            Self::Run { task, .. }
            | Self::PlanRun { task, .. }
            | Self::PlanReview { task, .. }
            | Self::ReviewRun { task, .. } => Some(task),
            Self::Pm => None,
        }
    }
}

/// Concrete session identity, not just a mutable pane id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBinding {
    pub session: String,
    pub endpoint: Option<PathBuf>,
    pub transcript: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveCursor {
    pub offset: u64,
    pub last_complete_line: u64,
    pub coverage: Coverage,
}

/// Durable progress for one adapter reading one concrete session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCursor {
    pub adapter: String,
    pub binding: SessionBinding,
    pub cursor: ObserveCursor,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    #[default]
    Complete,
    Truncated,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InboxState {
    /// Bounded owner obligations; absence on an existing recipient requires explicit indexing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_header: Option<OwnerHeader>,
    pub next_seq: u64,
    pub next_envelope: u64,
    pub open_envelope: Option<u64>,
    /// Retired unsent publication whose adapter still owns its pending replacement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement_envelope: Option<u64>,
    pub effects_applied: BTreeSet<u64>,
    pub binding: Option<SessionBinding>,
    /// Current-binding mirror; scoped cursors are authoritative for readers.
    pub observe_cursor: ObserveCursor,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub observe_cursors: Vec<SessionCursor>,
    /// When a closed Pi-native container's run-end duties settled (its messages terminalized and
    /// its runtime removed); sync never revisits a settled container.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled: Option<jiff::Timestamp>,
}

impl Default for InboxState {
    fn default() -> Self {
        Self {
            owner_header: None,
            next_seq: 1,
            next_envelope: 1,
            open_envelope: None,
            replacement_envelope: None,
            effects_applied: BTreeSet::new(),
            binding: None,
            observe_cursor: ObserveCursor::default(),
            observe_cursors: Vec::new(),
            settled: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmInbox {
    #[serde(default)]
    pub delivery: DeliveryMode,
    #[serde(default)]
    pub inbox: InboxState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageBinding {
    pub run: Option<String>,
    pub decision: Option<String>,
    pub review_round: Option<u32>,
    pub question_request: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Wake,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub recipient: Recipient,
    pub seq: u64,
    pub kind: String,
    pub urgency: Urgency,
    pub author: String,
    pub created: Timestamp,
    pub binding: MessageBinding,
    pub subject: Option<Value>,
    #[serde(skip)]
    pub body: String,
}

impl Message {
    pub fn owner_kind(&self) -> bool {
        header::owner_kind(&self.kind)
    }
}

pub struct Post<'a> {
    pub kind: &'a str,
    pub urgency: Urgency,
    pub author: &'a str,
    pub body: &'a str,
    pub binding: MessageBinding,
    pub subject: Option<Value>,
    pub max_payload: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub envelope: Option<u64>,
    pub binding: Option<SessionBinding>,
    pub observed_at: Timestamp,
    pub via: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptLevel {
    Admitted,
    Observed,
}
impl ReceiptLevel {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Observed => "observed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Cancelled,
    Failed,
    Abandoned,
}
impl TerminalState {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SendOutcome {
    Admitted {
        detail: Value,
    },
    Accepted,
    Held {
        reason: String,
    },
    Rejected {
        reason: String,
        retryable_after_fix: bool,
    },
    Uncertain {
        reason: String,
    },
    /// Outcome unknown and the transport refuses to settle it by retry (the key is bound to
    /// other arguments). Only an observation or the owner's `inbox recover` decision settles it.
    Conflicted {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Stage {
    Dispatching,
    Outcome,
    Read,
    Pasted,
    Enter { n: u32 },
    Observed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub at: Timestamp,
    pub outcome: Option<SendOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub number: u64,
    pub members: Vec<u64>,
    pub binding: SessionBinding,
    pub adapter: String,
    pub mode: String,
    pub payload_sha256: String,
    pub payload_path: PathBuf,
    pub idempotency_key: String,
    pub outcome: Option<SendOutcome>,
    pub stages: Vec<Stage>,
    pub attempts: Vec<Attempt>,
    /// Sticky proof survives public evidence rewriting and bounded owner-header pruning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_bar: Option<RetryWitness>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectStatus {
    Started,
    Applied,
    SkippedStale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    pub status: EffectStatus,
    pub receipt: Receipt,
    /// Exact answer prepared before any locked effect, used to prove replay ownership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_receipt: Option<crate::ResumeReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEntry {
    pub message: Message,
    pub admitted: Option<Receipt>,
    pub observed: Option<Receipt>,
    pub terminal: Option<TerminalState>,
    pub effect: Option<Effect>,
    pub envelope: Option<Envelope>,
}
