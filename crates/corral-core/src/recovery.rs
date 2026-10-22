//! Recovery evidence is not authority. Operations owns evaluation and owner confirmation.
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{CandidateIdentity, GateResult, HerdrRef, Verdict};

pub const RECOVERY_VERSION: u32 = 1;

/// The closed V1 exception set; arbitrary checks cannot become waivable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryCheck {
    ReviewerFamilyPolicy,
    ConfiguredReviewerBinding,
    IndependentPlanReview,
    GateOutcome,
    ReviewOutcome,
    ResourcePressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckDisposition {
    Satisfied,
    Overridden,
    Waived,
    NotApplicable,
}

/// Original observations remain distinct from their owner-authorized disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryObservation {
    Predicate { satisfied: bool },
    Gate { result: GateResult },
    Review { verdict: Option<Verdict> },
    Missing,
    Unverifiable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCheckRecord {
    pub check: RecoveryCheck,
    pub observed: RecoveryObservation,
    pub disposition: CheckDisposition,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerClass {
    Recoverable,
    Hard,
    Prerequisite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryBlocker {
    pub code: String,
    pub class: BlockerClass,
    pub observed: RecoveryObservation,
    pub evidence: Vec<String>,
    pub proposed_disposition: Option<CheckDisposition>,
    pub ordinary_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReviewer {
    pub profile: String,
    pub model: String,
    pub vendor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryReviewStage {
    PlanReview,
    CodeReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryAdmissionStage {
    Dispatch,
    PlanDispatch,
    Gate,
    CodeReview,
    PlanReview,
}

/// Binding an action never changes a persistent reviewer override or policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RecoveryAction {
    UseReviewer {
        stage: RecoveryReviewStage,
        reviewer: RecoveryReviewer,
    },
    AcceptPlan {
        waive_independent_review: bool,
        resolution_notes_sha256: String,
    },
    AcceptCode {
        merge: RecoveryMerge,
    },
    ForceAdmission {
        stage: RecoveryAdmissionStage,
        pressure_episode: String,
        thresholds_digest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMerge {
    Keep,
    Rebase,
    Ff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryIdentity {
    Plan {
        plan: String,
        artifact_sha256: String,
        review_attempt: Option<String>,
    },
    Code {
        candidate: CandidateIdentity,
        gate_attempt: Option<String>,
        review_attempt: Option<String>,
    },
    Admission {
        stage: RecoveryAdmissionStage,
        attempt: String,
        run: Option<String>,
        plan: Option<String>,
        candidate: Option<CandidateIdentity>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryTarget {
    pub store: PathBuf,
    pub task: String,
    pub task_revision: String,
    pub spec_revision: String,
    pub identity: RecoveryIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    pub version: u32,
    pub target: RecoveryTarget,
    pub action: RecoveryAction,
    pub exceptions: BTreeSet<RecoveryCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryResolutionFailure {
    pub code: String,
    pub message: String,
    pub evidence: Vec<String>,
}

/// Hash raw policy inputs without requiring reviewer selection to succeed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPolicy {
    pub digest: String,
    pub inputs: BTreeMap<String, serde_json::Value>,
    pub sources: BTreeMap<String, String>,
    pub expected_reviewer: Option<RecoveryReviewer>,
    pub resolution_failure: Option<RecoveryResolutionFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryEffect {
    ReviewAttempt {
        stage: RecoveryReviewStage,
        attempt: String,
        reviewer: RecoveryReviewer,
    },
    PlanAcceptance {
        plan: String,
        dispatch_holds: Vec<String>,
    },
    CodeAcceptance {
        candidate: CandidateIdentity,
        merge: RecoveryMerge,
    },
    Admission {
        stage: RecoveryAdmissionStage,
        attempt: String,
    },
}

/// Read-only evaluation output; its digest is an identity, never permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPreview {
    pub version: u32,
    pub request: RecoveryRequest,
    pub policy: RecoveryPolicy,
    pub checks: Vec<RecoveryCheckRecord>,
    pub blockers: Vec<RecoveryBlocker>,
    pub remaining_blockers: Vec<RecoveryBlocker>,
    pub proposed_effect: RecoveryEffect,
    pub affected_targets: Vec<String>,
    pub expires: Timestamp,
    pub digest: String,
    pub contract_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerInteraction {
    Cli,
    Tui,
}

/// Persisted attestation evidence, not an exported confirmation token or actor flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryAttestation {
    LocalOwnerAttestation {
        interaction: OwnerInteraction,
        at: Timestamp,
        controlling_terminal: String,
        context: Option<HerdrRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAuthorization {
    pub version: u32,
    pub preview_digest: String,
    pub reason: String,
    pub attestation: RecoveryAttestation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    Authorized,
    Applying,
    Applied,
    Failed,
    Unknown,
}

/// Outcomes are appended, never rewritten, including after uncertain delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryOutcome {
    pub state: RecoveryState,
    pub at: Timestamp,
    pub detail: String,
    pub actual_effect: Option<RecoveryEffect>,
    pub after: Option<RecoveryTarget>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReference {
    pub operation: String,
    pub contract_digest: String,
}

/// Exact receipt/journal anchors let downstream services retain the original data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryReceipt {
    PlanReview {
        plan: String,
        attempt: String,
    },
    PlanApproval {
        plan: String,
        at: Timestamp,
    },
    CodeReview {
        candidate: CandidateIdentity,
    },
    CodeAcceptance {
        candidate: CandidateIdentity,
    },
    LaunchJournal {
        stage: RecoveryAdmissionStage,
        attempt: String,
    },
    LandingJournal {
        run: String,
        candidate: CandidateIdentity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryLink {
    pub recovery: RecoveryReference,
    pub receipt: RecoveryReceipt,
    pub acceptance: Option<RecoveryAcceptance>,
}

/// Separate machine-consumed acceptance modes, including unmodified legacy history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceMode {
    Policy,
    OwnerOverride,
    OwnerRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAcceptance {
    pub acceptance_mode: AcceptanceMode,
    pub policy_satisfied: bool,
    pub recovery: Option<RecoveryReference>,
    pub reason: Option<String>,
    pub checks: Vec<RecoveryCheckRecord>,
}

impl RecoveryAcceptance {
    /// Keep the recorded disposition distinct from the original check outcome.
    ///
    /// # Errors
    /// Refuses fabricated policy success or recovery acceptance without provenance.
    pub fn validate(&self) -> Result<()> {
        let exception = self.checks.iter().any(|check| {
            matches!(
                check.disposition,
                CheckDisposition::Overridden | CheckDisposition::Waived
            )
        });
        ensure!(
            !exception || !self.policy_satisfied,
            "exceptions cannot satisfy ordinary policy"
        );
        match self.acceptance_mode {
            AcceptanceMode::Policy => {
                ensure!(
                    self.policy_satisfied && self.recovery.is_none(),
                    "invalid policy acceptance"
                );
            }
            AcceptanceMode::OwnerOverride => {
                // Missing historical reasons stay absent; never manufacture one.
                ensure!(
                    self.recovery.is_none(),
                    "legacy override cannot claim recovery authority"
                );
            }
            AcceptanceMode::OwnerRecovery => {
                ensure!(
                    self.recovery.is_some()
                        && self.reason.as_ref().is_some_and(|s| !s.trim().is_empty()),
                    "owner recovery acceptance requires its reference and reason"
                );
            }
        }
        Ok(())
    }
}

/// Authoritative intent and append-only effects in the canonical task envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRecord {
    pub version: u32,
    pub id: String,
    pub preview: RecoveryPreview,
    pub authorization: RecoveryAuthorization,
    pub outcomes: Vec<RecoveryOutcome>,
    pub links: Vec<RecoveryLink>,
}

impl RecoveryRecord {
    /// Validate the persistence boundary, not workflow eligibility or owner authority.
    ///
    /// # Errors
    /// Refuses unsupported versions, unbound intent, blank reasons or mismatched links.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            [
                self.version,
                self.preview.version,
                self.preview.request.version,
                self.authorization.version
            ]
            .iter()
            .all(|v| *v == RECOVERY_VERSION),
            "unsupported recovery version"
        );
        ensure!(!self.id.trim().is_empty(), "blank recovery identity");
        ensure!(
            !self.preview.digest.trim().is_empty(),
            "blank recovery preview digest"
        );
        ensure!(
            !self.preview.contract_digest.trim().is_empty(),
            "blank recovery contract digest"
        );
        ensure!(
            self.authorization.preview_digest == self.preview.digest,
            "recovery authorization does not bind the preview"
        );
        ensure!(
            !self.authorization.reason.trim().is_empty(),
            "blank recovery reason"
        );
        ensure!(
            self.outcomes
                .first()
                .is_some_and(|o| o.state == RecoveryState::Authorized),
            "recovery must retain its authorized intent"
        );
        for link in &self.links {
            ensure!(
                link.recovery.operation == self.id
                    && link.recovery.contract_digest == self.preview.contract_digest,
                "recovery receipt reference mismatch"
            );
            if let Some(acceptance) = &link.acceptance {
                acceptance.validate()?;
                ensure!(
                    acceptance.acceptance_mode == AcceptanceMode::OwnerRecovery
                        && acceptance.recovery.as_ref() == Some(&link.recovery),
                    "recovered receipt must retain recovery acceptance provenance"
                );
            }
        }
        Ok(())
    }

    /// Storage enforces append-only history; operations chooses each transition.
    ///
    /// # Errors
    /// Refuses changed intent, rewritten observations, removed outcomes or terminal replay.
    pub(crate) fn preserves(&self, previous: &Self) -> Result<()> {
        self.validate()?;
        ensure!(
            self.version == previous.version
                && self.id == previous.id
                && self.preview == previous.preview
                && self.authorization == previous.authorization,
            "recovery intent is immutable"
        );
        ensure!(
            self.outcomes.starts_with(&previous.outcomes)
                && self.links.starts_with(&previous.links),
            "recovery outcomes and receipt links are append-only"
        );
        if previous
            .outcomes
            .last()
            .is_some_and(|o| matches!(o.state, RecoveryState::Applied | RecoveryState::Failed))
        {
            ensure!(
                self.outcomes == previous.outcomes,
                "terminal recovery outcome is immutable"
            );
        }
        Ok(())
    }
}
