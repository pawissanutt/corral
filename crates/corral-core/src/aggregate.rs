//! Plan-scoped aggregate verification. Child task acceptance never implies these facts.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    ApprovedScope, CandidateIdentity, EvidenceRecord, GateRecord, HerdrRef, PlanDeliveryTarget,
    ReviewRoute, Task, Verdict,
};

/// The run name shared by every aggregate identity and action.
pub const AGGREGATE_RUN: &str = "aggregate";

/// One required member's accepted delivery, captured when an attempt starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateMember {
    pub task: String,
    pub run: String,
    pub contract_revision: String,
    /// Commit recorded by the member's acceptance receipt.
    pub receipt_sha: String,
    /// False when the accepted run added no commits beyond its dispatch base.
    pub code: bool,
    /// Accepted artifact versions of the member's run, sorted.
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateStrategy {
    /// The Plan integration branch fast-forwarded main.
    FastForward,
    /// The integration branch already equals main; nothing to land.
    AlreadyOnMain,
    /// Every member landed on main; acceptance records the exact head.
    Direct,
    /// No member changed code; acceptance binds accepted artifact versions.
    NoCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateLandingState {
    /// Durable intent written before any Git mutation.
    Prepared,
    Landed,
}

/// Final acceptance and its landing receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateAcceptance {
    pub authorization: String,
    pub automatic: bool,
    pub by: String,
    pub at: Timestamp,
    pub strategy: AggregateStrategy,
    /// Main before landing (code Plans only).
    pub before: Option<String>,
    /// Intended head while prepared; actual main afterwards (code Plans only).
    pub after: Option<String>,
    pub state: AggregateLandingState,
}

/// One launched review of an aggregate attempt. The selected reviewer, candidate and evidence
/// are frozen before launch; only the callback naming this exact review ID can complete it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateReview {
    /// `<attempt>-R<n>`, unique within the attempt.
    pub id: String,
    /// The resolved reviewer profile, or "pm", chosen by the effective review policy.
    pub reviewer: String,
    /// How the policy selected the reviewer (contract, profile revision, `auto` mapping).
    pub route: Box<ReviewRoute>,
    pub identity: CandidateIdentity,
    /// Digest of the attempt's evidence records the reviewer was asked to judge.
    pub evidence_digest: String,
    pub requested: Timestamp,
    /// The pane the reviewer was launched in; absent until the launch is attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Verdict time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
    /// Why the review ended without a verdict (superseded or failed launch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandoned: Option<String>,
}

impl AggregateReview {
    /// Launched and answered through its own callback.
    pub fn completed(&self) -> bool {
        self.herdr.is_some() && self.verdict.is_some() && self.abandoned.is_none()
    }

    /// Launched (or launching) and still waiting for its callback.
    pub fn pending(&self) -> bool {
        self.verdict.is_none() && self.abandoned.is_none()
    }
}

/// One verification attempt of the combined candidate. Failed attempts remain history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateAttempt {
    pub id: String,
    pub identity: CandidateIdentity,
    pub target: PlanDeliveryTarget,
    pub code: bool,
    pub members: Vec<AggregateMember>,
    pub started: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRecord>,
    /// Launched aggregate reviews, oldest first; the last one is current.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<AggregateReview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<AggregateAcceptance>,
}

impl ApprovedScope {
    /// Stable aggregate criterion keys: the criterion ID, else the one-based position.
    pub fn criterion_keys(&self) -> Vec<String> {
        self.acceptance
            .iter()
            .enumerate()
            .map(|(index, acceptance)| {
                acceptance
                    .criterion
                    .clone()
                    .unwrap_or_else(|| format!("#{}", index + 1))
            })
            .collect()
    }
}

impl Task {
    /// A tracker with approved acceptance criteria completes only through aggregate acceptance.
    /// Plain trackers keep completing when every child is Done.
    pub fn plan_acceptance_required(&self) -> bool {
        self.tracker
            && self
                .approved_scope
                .as_ref()
                .is_some_and(|scope| !scope.acceptance.is_empty())
    }

    /// The accepted attempt, prepared or landed.
    pub fn aggregate_acceptance(&self) -> Option<&AggregateAttempt> {
        self.aggregate_attempts
            .iter()
            .find(|attempt| attempt.acceptance.is_some())
    }
}
