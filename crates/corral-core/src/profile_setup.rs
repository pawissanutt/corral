//! Version 1 setup records. These describe approval of configuration, never execution authority.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use jiff::{Timestamp, civil::Date};
use serde::{Deserialize, Serialize};

use crate::{Config, JsonPath, PathProblems, Profile, ProfileRole};

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
        let mut problems = PathProblems::default();
        self.collect_problems(config, &JsonPath::default(), &mut problems);
        problems.first()
    }

    /// Every integrity and configuration problem, each at its own path below `base`.
    pub fn collect_problems(&self, config: &Config, base: &JsonPath, problems: &mut PathProblems) {
        self.integrity_problems(base, problems);
        self.configuration_problems(config, base, problems);
    }

    fn validate_configuration(&self, config: &Config) -> Result<()> {
        let mut problems = PathProblems::default();
        self.configuration_problems(config, &JsonPath::default(), &mut problems);
        problems.first()
    }

    fn validate_integrity(&self) -> Result<()> {
        let mut problems = PathProblems::default();
        self.integrity_problems(&JsonPath::default(), &mut problems);
        problems.first()
    }

    fn configuration_problems(&self, config: &Config, base: &JsonPath, out: &mut PathProblems) {
        let profiles_path = base.key("profiles");
        let mut proposed = config.clone();
        proposed.profiles.extend(self.profiles.clone());
        let problems = proposed.profile_problems();
        out.require(
            problems.is_empty(),
            &profiles_path,
            &[],
            format!(
                "invalid proposed profiles: {}",
                crate::profile::describe_profile_problems(&problems)
            ),
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
        out.require(
            crate::config_validation::escalation_problems(&names, &targets, true).is_empty(),
            &profiles_path,
            &[],
            "invalid proposed escalation chain",
        );
        for (role, key) in &self.roles {
            if let Err(err) = proposed.check_profile_eligibility(key, *role, None, false) {
                let role = serde_json::to_value(role)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();
                out.error(
                    &base.key("roles").key(&role),
                    &[&profiles_path.key(key)],
                    &err,
                );
            }
        }
        for (key, recommendation) in &self.recommendations {
            let path = base.key("recommendations").key(key);
            for (index, alternative) in recommendation.alternatives_considered.iter().enumerate() {
                if let Err(err) = alternative.validate(&proposed) {
                    out.error(&path.key("alternatives_considered").index(index), &[], &err);
                }
            }
        }
    }

    fn integrity_problems(&self, base: &JsonPath, out: &mut PathProblems) {
        out.require(
            self.version == PROFILE_RECORD_VERSION,
            &base.key("version"),
            &[],
            "unsupported setup proposal version",
        );
        for (field, value) in [
            ("id", &self.id),
            ("procedure_version", &self.procedure_version),
            ("based_on_config_revision", &self.based_on_config_revision),
        ] {
            out.require(
                !value.trim().is_empty(),
                &base.key(field),
                &[],
                "setup proposal identity is empty",
            );
        }
        let profiles_path = base.key("profiles");
        out.require(
            !self.profiles.is_empty(),
            &profiles_path,
            &[],
            "setup proposal has no profiles",
        );
        let evidence_path = base.key("evidence");
        for (key, evidence) in &self.evidence {
            let path = evidence_path.key(key);
            out.require(
                key == &evidence.id && !key.trim().is_empty(),
                &path.key("id"),
                &[],
                "broken evidence identity",
            );
            out.require(
                evidence.version == PROFILE_RECORD_VERSION,
                &path.key("version"),
                &[],
                "unsupported evidence version",
            );
            let source = match &evidence.source {
                ModelEvidenceSource::Web { url } => {
                    out.require(
                        url.starts_with("https://") || url.starts_with("http://"),
                        &path.key("source"),
                        &[],
                        "invalid evidence URL",
                    );
                    url
                }
                ModelEvidenceSource::LocalDiscovery { method } => method,
            };
            out.require(
                !source.trim().is_empty()
                    && !evidence.claim.trim().is_empty()
                    && !evidence.finding.trim().is_empty(),
                &path,
                &[],
                "empty evidence source/claim/finding",
            );
        }
        let check_evidence = |out: &mut PathProblems, id: &str, path: &JsonPath| {
            out.require(
                self.evidence.contains_key(id),
                path,
                &[&evidence_path.key(id)],
                format!("unknown evidence '{id}'"),
            );
        };
        let recommendations_path = base.key("recommendations");
        for (key, profile) in &self.profiles {
            let path = profiles_path.key(key);
            out.require(!key.trim().is_empty(), &path, &[], "empty profile key");
            match &profile.metadata {
                None => out.push(
                    &path.key("metadata"),
                    &[],
                    "proposed profile needs catalog metadata",
                ),
                Some(metadata) => {
                    let path = path.key("metadata");
                    if let Some(evidence) = &metadata.evidence {
                        check_evidence(out, evidence, &path.key("evidence"));
                    }
                    for (index, candidate) in metadata.candidates.iter().enumerate() {
                        let path = path.key("candidates").index(index).key("evidence");
                        check_evidence(out, &candidate.evidence, &path);
                    }
                    if let Some(fallback) = &metadata.fallback {
                        // Approval is a reference to this exact proposal's approval record.
                        out.require(
                            fallback.approval == self.id,
                            &path.key("fallback").key("approval"),
                            &[&base.key("id")],
                            "fallback approval names another proposal",
                        );
                    }
                }
            }
            let recommendation_path = recommendations_path.key(key);
            let Some(recommendation) = self.recommendations.get(key) else {
                out.push(
                    &recommendation_path,
                    &[&path],
                    "missing profile recommendation",
                );
                continue;
            };
            out.require(
                !recommendation.reason.trim().is_empty(),
                &recommendation_path.key("reason"),
                &[],
                "empty recommendation reason",
            );
            out.require(
                !matches!(self.basis, SetupBasis::Sourced) || !recommendation.evidence.is_empty(),
                &recommendation_path.key("evidence"),
                &[&base.key("basis")],
                "sourced recommendation lacks evidence",
            );
            for (index, id) in recommendation.evidence.iter().enumerate() {
                check_evidence(out, id, &recommendation_path.key("evidence").index(index));
            }
            for (index, alternative) in recommendation.alternatives_considered.iter().enumerate() {
                let path = recommendation_path
                    .key("alternatives_considered")
                    .index(index)
                    .key("evidence");
                check_evidence(out, &alternative.evidence, &path);
            }
        }
        for key in self.recommendations.keys() {
            out.require(
                self.profiles.contains_key(key),
                &recommendations_path.key(key),
                &[&profiles_path.key(key)],
                "recommendation names an unchanged profile",
            );
        }
        for (index, access) in self.access.iter().enumerate() {
            let path = base.key("access").index(index);
            let profile_path = profiles_path.key(&access.profile);
            match self.profiles.get(&access.profile) {
                None => out.push(
                    &path.key("profile"),
                    &[&profile_path],
                    "access names unknown profile",
                ),
                Some(profile) => out.require(
                    profile.metadata.as_ref().is_some_and(|m| {
                        m.candidates
                            .iter()
                            .any(|candidate| candidate.id == access.candidate)
                    }),
                    &path.key("candidate"),
                    &[&profile_path],
                    "access names unknown candidate",
                ),
            }
            for (item, id) in access.evidence.iter().enumerate() {
                check_evidence(out, id, &path.key("evidence").index(item));
            }
        }
        if let SetupBasis::Provisional { exception } = &self.basis {
            out.require(
                !exception.trim().is_empty(),
                &base.key("basis").key("exception"),
                &[],
                "provisional exception is empty",
            );
        }
        if let Some(policy) = &self.repair_policy
            && let Err(err) = policy.validate()
        {
            out.error(&base.key("repair_policy"), &[], &err);
        }
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
