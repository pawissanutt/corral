//! Version 1 setup records. These describe approval of configuration, never execution authority.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use jiff::{Timestamp, civil::Date};
use serde::{Deserialize, Serialize};

use crate::{Config, Profile, ProfileRole};

pub const PROFILE_RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelEvidenceSource {
    Web { url: String },
    LocalDiscovery { method: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEvidence {
    pub version: u32,
    pub id: String,
    pub source: ModelEvidenceSource,
    pub retrieved_at: Timestamp,
    pub published_on: Option<Date>,
    pub measured_on: Option<Date>,
    pub claim: String,
    pub finding: String,
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// Independent observations, not a single misleading "verified" bit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccessFacets {
    pub installed: bool,
    pub discovered: bool,
    pub user_reported: bool,
    pub access_verified: bool,
    pub execution_tested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateAccess {
    pub profile: String,
    pub candidate: String,
    pub observed_at: Timestamp,
    pub facets: AccessFacets,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupPreferences {
    pub quality_cost_preference: Option<String>,
    pub allowed_providers: Vec<String>,
    pub provider_preference: Vec<String>,
    pub spending: Option<crate::SpendingBound>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recommendation {
    pub reason: String,
    pub evidence: Vec<String>,
    pub alternatives_considered: Vec<crate::ExecutionCandidate>,
    pub uncertainty: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetupBasis {
    Sourced,
    Provisional {
        exception: String,
    },
    /// Explicitly manual is not evidence-verified.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSetupProposal {
    pub version: u32,
    pub id: String,
    pub prepared_at: Timestamp,
    pub procedure_version: String,
    pub based_on_config_revision: String,
    pub preferences: SetupPreferences,
    pub basis: SetupBasis,
    /// Targeted changes, not a replacement of unrelated configuration.
    pub profiles: BTreeMap<String, Profile>,
    pub roles: BTreeMap<ProfileRole, String>,
    pub evidence: BTreeMap<String, ModelEvidence>,
    pub recommendations: BTreeMap<String, Recommendation>,
    pub access: Vec<CandidateAccess>,
    /// Absent preserves the current repair policy.
    pub repair_policy: Option<crate::RepairPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupApprover {
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSetupApproval {
    pub version: u32,
    pub id: String,
    pub by: SetupApprover,
    pub at: Timestamp,
    pub proposal: String,
    pub proposal_digest: String,
    pub based_on_config_revision: String,
    pub acknowledged_evidence_gap: bool,
}

impl ProfileSetupProposal {
    pub fn digest(&self) -> Result<String> {
        crate::operating::digest(self)
    }

    pub fn validate(&self, config: &Config) -> Result<()> {
        self.validate_integrity()?;
        self.validate_configuration(config)
    }

    fn validate_configuration(&self, config: &Config) -> Result<()> {
        let mut proposed = config.clone();
        proposed.profiles.extend(self.profiles.clone());
        ensure!(
            proposed.profile_problems().is_empty(),
            "invalid proposed profiles"
        );
        let names = proposed.profile_names().into_iter().collect();
        let targets = proposed
            .profiles
            .iter()
            .filter_map(|(key, profile)| {
                profile
                    .escalate_to
                    .as_ref()
                    .map(|target| (key.clone(), target.clone()))
            })
            .collect();
        ensure!(
            crate::config_validation::escalation_problems(&names, &targets, true).is_empty(),
            "invalid proposed escalation chain"
        );
        for (role, key) in &self.roles {
            proposed.check_profile_eligibility(key, *role, None, false)?;
        }
        for recommendation in self.recommendations.values() {
            for alternative in &recommendation.alternatives_considered {
                alternative.validate(&proposed)?;
            }
        }
        Ok(())
    }

    fn validate_integrity(&self) -> Result<()> {
        ensure!(
            self.version == PROFILE_RECORD_VERSION,
            "unsupported setup proposal version"
        );
        ensure!(
            !self.id.trim().is_empty()
                && !self.procedure_version.trim().is_empty()
                && !self.based_on_config_revision.trim().is_empty(),
            "setup proposal identity is empty"
        );
        ensure!(!self.profiles.is_empty(), "setup proposal has no profiles");
        for (key, evidence) in &self.evidence {
            ensure!(
                key == &evidence.id && !key.trim().is_empty(),
                "broken evidence identity"
            );
            ensure!(
                evidence.version == PROFILE_RECORD_VERSION,
                "unsupported evidence version"
            );
            let source = match &evidence.source {
                ModelEvidenceSource::Web { url } => {
                    ensure!(
                        url.starts_with("https://") || url.starts_with("http://"),
                        "invalid evidence URL"
                    );
                    url
                }
                ModelEvidenceSource::LocalDiscovery { method } => method,
            };
            ensure!(
                !source.trim().is_empty()
                    && !evidence.claim.trim().is_empty()
                    && !evidence.finding.trim().is_empty(),
                "empty evidence source/claim/finding"
            );
        }
        let check_evidence = |id: &str| -> Result<()> {
            ensure!(self.evidence.contains_key(id), "unknown evidence '{id}'");
            Ok(())
        };
        for (key, profile) in &self.profiles {
            ensure!(!key.trim().is_empty(), "empty profile key");
            let metadata = profile
                .metadata
                .as_ref()
                .context("proposed profile needs catalog metadata")?;
            if let Some(evidence) = &metadata.evidence {
                check_evidence(evidence)?;
            }
            for candidate in &metadata.candidates {
                check_evidence(&candidate.evidence)?;
            }
            if let Some(fallback) = &metadata.fallback {
                // Approval is a reference to this exact proposal's approval record.
                ensure!(
                    fallback.approval == self.id,
                    "fallback approval names another proposal"
                );
            }
            let recommendation = self
                .recommendations
                .get(key)
                .context("missing profile recommendation")?;
            ensure!(
                !recommendation.reason.trim().is_empty(),
                "empty recommendation reason"
            );
            if matches!(self.basis, SetupBasis::Sourced) {
                ensure!(
                    !recommendation.evidence.is_empty(),
                    "sourced recommendation lacks evidence"
                );
            }
            for id in &recommendation.evidence {
                check_evidence(id)?;
            }
            for alternative in &recommendation.alternatives_considered {
                check_evidence(&alternative.evidence)?;
            }
        }
        ensure!(
            self.recommendations
                .keys()
                .all(|key| self.profiles.contains_key(key)),
            "recommendation names an unchanged profile"
        );
        for access in &self.access {
            let profile = self
                .profiles
                .get(&access.profile)
                .context("access names unknown profile")?;
            ensure!(
                profile.metadata.as_ref().is_some_and(|m| m
                    .candidates
                    .iter()
                    .any(|candidate| candidate.id == access.candidate)),
                "access names unknown candidate"
            );
            for id in &access.evidence {
                check_evidence(id)?;
            }
        }
        if let SetupBasis::Provisional { exception } = &self.basis {
            ensure!(
                !exception.trim().is_empty(),
                "provisional exception is empty"
            );
        }
        if let Some(policy) = &self.repair_policy {
            policy.validate()?;
        }
        Ok(())
    }

    pub fn check_approval(&self, approval: &ProfileSetupApproval) -> Result<()> {
        ensure!(
            approval.version == PROFILE_RECORD_VERSION && !approval.id.trim().is_empty(),
            "invalid setup approval identity/version"
        );
        ensure!(
            approval.proposal == self.id
                && approval.proposal_digest == self.digest()?
                && approval.based_on_config_revision == self.based_on_config_revision,
            "approval does not bind this exact proposal"
        );
        ensure!(
            approval.at >= self.prepared_at,
            "approval predates proposal"
        );
        if matches!(self.basis, SetupBasis::Provisional { .. }) {
            ensure!(
                approval.acknowledged_evidence_gap,
                "provisional setup requires owner acknowledgement"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileSetupState {
    #[default]
    NotStarted,
    Gathering,
    Proposed,
    Applied,
    Cancelled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupValidationFacets {
    pub configuration_valid: bool,
    pub access_checked: bool,
    pub execution_tested: bool,
}

/// Single atomic document keeps evidence available before any approval references it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSetupRecord {
    pub version: u32,
    pub revision: u64,
    pub state: ProfileSetupState,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    pub proposal: Option<ProfileSetupProposal>,
    pub approval: Option<ProfileSetupApproval>,
    pub applied_config_revision: Option<String>,
    pub validation: SetupValidationFacets,
}

impl Default for ProfileSetupRecord {
    fn default() -> Self {
        Self {
            version: PROFILE_RECORD_VERSION,
            revision: 0,
            state: ProfileSetupState::NotStarted,
            answers: BTreeMap::new(),
            proposal: None,
            approval: None,
            applied_config_revision: None,
            validation: SetupValidationFacets::default(),
        }
    }
}

impl ProfileSetupRecord {
    pub fn validate(&self, config: &Config) -> Result<()> {
        self.validate_integrity()?;
        if let Some(proposal) = &self.proposal {
            proposal.validate_configuration(config)?;
        }
        Ok(())
    }

    pub(crate) fn validate_integrity(&self) -> Result<()> {
        ensure!(
            self.version == PROFILE_RECORD_VERSION,
            "unsupported setup record version"
        );
        if let Some(proposal) = &self.proposal {
            proposal.validate_integrity()?;
        }
        if let Some(approval) = &self.approval {
            self.proposal
                .as_ref()
                .context("approval without proposal")?
                .check_approval(approval)?;
        }
        match self.state {
            ProfileSetupState::NotStarted | ProfileSetupState::Gathering => {
                ensure!(
                    self.proposal.is_none() && self.approval.is_none(),
                    "unexpected proposal/approval"
                );
            }
            ProfileSetupState::Proposed => ensure!(self.proposal.is_some(), "missing proposal"),
            ProfileSetupState::Applied => {
                ensure!(
                    self.proposal.is_some()
                        && self.approval.is_some()
                        && self.validation.configuration_valid,
                    "applied setup lacks validated approval"
                );
                ensure!(
                    self.applied_config_revision
                        .as_ref()
                        .is_some_and(|r| !r.trim().is_empty()),
                    "applied setup lacks configuration revision"
                );
            }
            ProfileSetupState::Cancelled => {
                ensure!(self.approval.is_none(), "cancelled proposal is approved")
            }
        }
        if self.state != ProfileSetupState::Applied {
            ensure!(
                self.applied_config_revision.is_none(),
                "unapplied setup has applied revision"
            );
        }
        Ok(())
    }

    pub fn check_transition(&self, next: &Self) -> Result<()> {
        use ProfileSetupState::*;
        ensure!(
            next.revision
                == self
                    .revision
                    .checked_add(1)
                    .context("setup revision exhausted")?,
            "stale setup revision"
        );
        let allowed = match (self.state, next.state) {
            (NotStarted, Gathering)
            | (Gathering, Gathering | Proposed | Cancelled)
            | (Proposed, Proposed | Applied | Cancelled)
            | (Applied | Cancelled, Gathering) => true,
            (NotStarted, NotStarted | Proposed | Applied | Cancelled)
            | (Gathering, NotStarted | Applied)
            | (Proposed, NotStarted | Gathering)
            | (Applied | Cancelled, NotStarted | Proposed | Applied | Cancelled) => false,
        };
        ensure!(allowed, "invalid setup state transition");
        if next.state == Applied {
            ensure!(
                self.proposal == next.proposal && self.approval == next.approval,
                "apply must preserve the approved proposal"
            );
        }
        Ok(())
    }
}
