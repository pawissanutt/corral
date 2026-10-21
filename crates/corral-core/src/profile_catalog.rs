//! Stable catalog metadata. Keys live in `Config::profiles`, never in display labels.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{Config, Profile};

/// Editable starter identities, not a closed set of accepted profile keys.
pub const STARTER_PROFILE_KEYS: [&str; 9] = [
    "general",
    "quick",
    "frontend",
    "deep",
    "planning",
    "writing",
    "implementation-review",
    "design-review",
    "visual-review",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRole {
    Implementation,
    Planning,
    Pm,
    PlanReview,
    ImplementationReview,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionGuidance {
    pub use_when: Vec<String>,
    pub avoid_when: Vec<String>,
    pub positive_examples: Vec<String>,
    pub negative_examples: Vec<String>,
    pub distinctions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileEligibility {
    pub enabled: bool,
    pub automatic: bool,
    pub roles: BTreeSet<ProfileRole>,
    /// Empty means all configured projects, not permission to add projects.
    #[serde(default)]
    pub projects: BTreeSet<String>,
}

impl ProfileEligibility {
    pub fn allows(&self, role: ProfileRole, project: Option<&str>, automatic: bool) -> bool {
        self.enabled
            && (!automatic || self.automatic)
            && self.roles.contains(&role)
            && (self.projects.is_empty() || project.is_some_and(|p| self.projects.contains(p)))
    }
}

/// Each candidate has its own settings; nothing is inherited from a previous candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCandidate {
    pub id: String,
    pub harness: String,
    pub connection: String,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub evidence: String,
    /// API-equivalent price is not subscription spending.
    pub cost: CandidateCost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CandidateCost {
    Unknown,
    Subscription,
    Metered {
        currency: String,
        maximum_micros: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvailabilityFallback {
    pub automatic: bool,
    pub approved_providers: BTreeSet<String>,
    /// Maximum advances, excluding the preferred candidate. Never cycles.
    pub max_switches: u32,
    pub allow_unknown_subscription_cost: bool,
    pub spending: Option<SpendingBound>,
    /// Proposal identity whose exact contents must have an owner approval receipt.
    pub approval: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpendingBound {
    pub currency: String,
    pub maximum_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMetadata {
    pub display_name: String,
    pub purpose: String,
    pub guidance: SelectionGuidance,
    pub instructions: String,
    pub eligibility: ProfileEligibility,
    pub revision: String,
    /// None explicitly means manually configured, not evidence verified.
    pub evidence: Option<String>,
    /// Preferred first, then ordered alternatives. Empty keeps the legacy binding.
    #[serde(default)]
    pub candidates: Vec<ExecutionCandidate>,
    pub fallback: Option<AvailabilityFallback>,
}

impl ExecutionCandidate {
    pub fn validate(&self, config: &Config) -> Result<()> {
        for value in [
            &self.id,
            &self.connection,
            &self.provider,
            &self.model,
            &self.evidence,
        ] {
            ensure!(
                !value.trim().is_empty(),
                "candidate identity/settings/evidence is empty"
            );
        }
        let (command, flags) = config.harness(&self.harness)?;
        ensure!(
            crate::infer_vendor(&self.model).is_none_or(|vendor| vendor == self.provider),
            "candidate provider disagrees with model identity"
        );
        flags.render(&self.harness, Some(&self.model), self.effort.as_deref())?;
        ensure!(
            command.contains("{model_args}"),
            "candidate harness lacks model_args"
        );
        ensure!(
            !self.args.iter().any(|arg| arg.contains('\0')),
            "candidate argv contains NUL"
        );
        if let CandidateCost::Metered { currency, .. } = &self.cost {
            ensure!(!currency.trim().is_empty(), "candidate currency is empty");
        }
        Ok(())
    }
}

impl ProfileMetadata {
    pub fn validate(&self, config: &Config, profile: &Profile) -> Result<()> {
        for value in [&self.display_name, &self.purpose, &self.revision] {
            ensure!(
                !value.trim().is_empty(),
                "profile name/purpose/revision is empty"
            );
        }
        ensure!(
            !self.eligibility.roles.is_empty(),
            "profile has no eligible roles"
        );
        for project in &self.eligibility.projects {
            ensure!(
                config.projects.contains_key(project),
                "unknown profile project '{project}'"
            );
        }
        ensure!(
            self.evidence.as_ref().is_none_or(|e| !e.trim().is_empty()),
            "empty evidence reference"
        );
        let mut ids = BTreeSet::new();
        for candidate in &self.candidates {
            candidate
                .validate(config)
                .with_context(|| format!("candidate '{}'", candidate.id))?;
            ensure!(
                ids.insert(&candidate.id),
                "duplicate execution candidate '{}'",
                candidate.id
            );
        }
        if let Some(preferred) = self.candidates.first() {
            ensure!(
                preferred.harness == profile.harness
                    && Some(&preferred.model) == profile.model.as_ref()
                    && preferred.effort == profile.effort
                    && preferred.args == profile.args,
                "preferred candidate differs from active profile binding"
            );
            ensure!(
                profile
                    .vendor
                    .as_ref()
                    .is_none_or(|v| v == &preferred.provider),
                "preferred candidate provider differs from profile vendor"
            );
        }
        if self.candidates.len() > 1 {
            ensure!(
                self.fallback.is_some(),
                "alternative candidates require approved fallback bounds"
            );
        }
        if let Some(fallback) = &self.fallback {
            ensure!(
                !fallback.approval.trim().is_empty(),
                "fallback approval is empty"
            );
            let switches = self.candidates.len().saturating_sub(1);
            ensure!(
                usize::try_from(fallback.max_switches)? <= switches,
                "fallback switch bound exceeds finite candidate list"
            );
            ensure!(
                !fallback.automatic || fallback.max_switches > 0,
                "automatic fallback has no allowance"
            );
            if let Some(bound) = &fallback.spending {
                ensure!(
                    !bound.currency.trim().is_empty(),
                    "spending currency is empty"
                );
            }
            for candidate in &self.candidates {
                fallback.admits(candidate)?;
            }
        }
        Ok(())
    }
}

impl Config {
    /// Legacy/manual profiles retain their existing unrestricted role behavior.
    pub fn check_profile_eligibility(
        &self,
        key: &str,
        role: ProfileRole,
        project: Option<&str>,
        automatic: bool,
    ) -> Result<()> {
        ensure!(
            self.profile_names().iter().any(|name| name == key),
            "unknown profile '{key}'"
        );
        if let Some(project) = project {
            ensure!(
                self.projects.contains_key(project),
                "unknown project '{project}'"
            );
        }
        if let Some(metadata) = self.profiles.get(key).and_then(|p| p.metadata.as_ref()) {
            ensure!(
                metadata.eligibility.allows(role, project, automatic),
                "profile '{key}' is ineligible"
            );
        }
        Ok(())
    }

    /// Guidance edits change this identity. Active/historical runs are not modified.
    pub fn profile_catalog_revision(&self) -> Result<String> {
        crate::operating::digest(&(&self.profiles, &self.agents))
    }
}
