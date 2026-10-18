//! Revision-bound selection receipts. Routing/launch integration is deliberately separate.
use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    CandidateIdentity, Config, PROFILE_RECORD_VERSION, PolicySource, ProfileRole, Reviewer, Task,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionActor {
    Proposer,
    Pm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewContractKind {
    Plan,
    Design,
    Implementation,
    ComplexImplementation,
    VisualImplementation,
    Prose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewContract {
    pub kind: ReviewContractKind,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProfileReviewSubject {
    Plan {
        task: String,
        plan: String,
        spec_revision: String,
        attempt: String,
    },
    Implementation {
        candidate: Box<CandidateIdentity>,
    },
}

impl ProfileReviewSubject {
    pub fn role(&self) -> ProfileRole {
        match self {
            Self::Plan { .. } => ProfileRole::PlanReview,
            Self::Implementation { .. } => ProfileRole::ImplementationReview,
        }
    }

    fn task(&self) -> &str {
        match self {
            Self::Plan { task, .. } => task,
            Self::Implementation { candidate } => &candidate.task,
        }
    }
}

/// Saves what the existing owner policy meant, not just the chosen profile name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerPolicyRecord {
    pub source: PolicySource,
    pub selector: String,
    pub require_cross_family: bool,
    pub policy_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileReviewSelection {
    pub version: u32,
    pub profile: String,
    pub profile_revision: String,
    pub contract: ReviewContract,
    pub subject: ProfileReviewSubject,
    pub policy: ReviewerPolicyRecord,
    pub reason: String,
}

impl ProfileReviewSelection {
    /// Uses the recorded planner for a plan; implementation review uses the author vendor.
    pub fn validate(&self, config: &Config, context: &ReviewSelectionContext<'_>) -> Result<()> {
        let planner = match &self.subject {
            ProfileReviewSubject::Plan { .. } => crate::plan_policy::planner(context.task)?
                .map(crate::plan_policy::PlannerProfile::from),
            ProfileReviewSubject::Implementation { .. } => None,
        };
        self.validate_with_planner(config, context, planner)
    }

    /// Preview may supply a prospective planner before a planning run is recorded.
    pub fn validate_with_planner(
        &self,
        config: &Config,
        context: &ReviewSelectionContext<'_>,
        planner: Option<crate::plan_policy::PlannerProfile<'_>>,
    ) -> Result<()> {
        ensure!(
            &self.subject == context.subject && self.subject.task() == context.task.id,
            "review selection names a different object/attempt"
        );
        if matches!(&self.subject, ProfileReviewSubject::Plan { .. })
            && let Some(recorded) = crate::plan_policy::planner(context.task)?
        {
            ensure!(
                planner.is_some_and(|p| p.profile == recorded.profile
                    && p.model == recorded.model.as_deref()
                    && p.vendor == recorded.vendor.as_deref()),
                "review planner context differs from recorded planner"
            );
        }
        self.validate_contract(context.task)?;
        self.validate_policy(config, context, planner)
    }

    fn validate_contract(&self, task: &Task) -> Result<()> {
        ensure!(
            self.version == PROFILE_RECORD_VERSION && self.contract.version == 1,
            "unsupported review selection/contract version"
        );
        match (&self.subject, self.contract.kind) {
            (
                ProfileReviewSubject::Plan {
                    plan,
                    spec_revision,
                    attempt,
                    ..
                },
                ReviewContractKind::Plan | ReviewContractKind::Design,
            ) => {
                ensure!(
                    !plan.trim().is_empty()
                        && !attempt.trim().is_empty()
                        && spec_revision == &crate::plan_contract_revision(task)?,
                    "stale or incomplete plan review identity"
                );
            }
            (
                ProfileReviewSubject::Implementation { .. },
                ReviewContractKind::Implementation
                | ReviewContractKind::ComplexImplementation
                | ReviewContractKind::VisualImplementation
                | ReviewContractKind::Prose
                | ReviewContractKind::Design,
            ) => {}
            (
                ProfileReviewSubject::Plan { .. },
                ReviewContractKind::Implementation
                | ReviewContractKind::ComplexImplementation
                | ReviewContractKind::VisualImplementation
                | ReviewContractKind::Prose,
            )
            | (ProfileReviewSubject::Implementation { .. }, ReviewContractKind::Plan) => {
                anyhow::bail!("review contract does not match plan/implementation role");
            }
        }
        ensure!(
            !self.reason.trim().is_empty(),
            "review selection reason is empty"
        );
        Ok(())
    }

    fn validate_policy(
        &self,
        config: &Config,
        context: &ReviewSelectionContext<'_>,
        planner: Option<crate::plan_policy::PlannerProfile<'_>>,
    ) -> Result<()> {
        config.check_profile_eligibility(
            &self.profile,
            self.subject.role(),
            context.task.project.as_deref(),
            false,
        )?;
        let revision = config
            .profiles
            .get(&self.profile)
            .and_then(|p| p.metadata.as_ref())
            .map(|m| m.revision.as_str())
            .unwrap_or("legacy");
        ensure!(
            revision == self.profile_revision,
            "stale reviewer profile revision"
        );
        let policy = config.task_review_policy(context.task);
        let source = if context.task.reviewer.is_some() {
            PolicySource::Task
        } else {
            config
                .review_policy_sources(context.task.project.as_deref(), context.task.review)
                .reviewer
        };
        ensure!(
            self.policy.source == source
                && self.policy.selector == reviewer_selector(&policy.reviewer)
                && self.policy.require_cross_family == policy.cross_family_required()
                && self.policy.policy_digest == reviewer_policy_digest(&policy)?,
            "stale or forged reviewer policy provenance"
        );
        match &self.subject {
            ProfileReviewSubject::Plan { .. } => {
                let expected = crate::plan_policy::expected_reviewer_for(
                    config,
                    context.task,
                    planner,
                    context.author_vendor,
                )?;
                let profile = if expected == "pm" {
                    config.pm.agent.as_str()
                } else {
                    &expected
                };
                ensure!(self.profile == profile, "plan reviewer policy mismatch");
                return crate::plan_policy::independent_reviewer_for(
                    config,
                    context.task,
                    planner,
                    &expected,
                );
            }
            ProfileReviewSubject::Implementation { .. } => {
                ensure!(
                    planner.is_none(),
                    "implementation review has planner context"
                );
            }
        }
        match &policy.reviewer {
            Reviewer::None => anyhow::bail!("review policy disables reviewer selection"),
            Reviewer::Pm => ensure!(
                self.profile == config.pm.agent,
                "PM reviewer policy mismatch"
            ),
            Reviewer::Profile(key) => {
                ensure!(key == &self.profile, "explicit reviewer policy mismatch")
            }
            Reviewer::CrossVendor => {
                let author = context.author_vendor.context("unknown author vendor")?;
                ensure!(
                    config.cross_vendor_reviewer(author)? == self.profile,
                    "cross-vendor reviewer policy mismatch"
                );
            }
        }
        let resolved = config.resolve_profile(
            &crate::ProfileChoice {
                profile: Some(self.profile.clone()),
                ..Default::default()
            },
            None,
            None,
        )?;
        policy.check_reviewer_family(context.author_vendor, resolved.vendor.as_deref())?;
        Ok(())
    }
}

pub struct ReviewSelectionContext<'a> {
    pub task: &'a Task,
    pub subject: &'a ProfileReviewSubject,
    pub author_vendor: Option<&'a str>,
}

pub fn reviewer_selector(reviewer: &Reviewer) -> String {
    match reviewer {
        Reviewer::None => "none".into(),
        Reviewer::Pm => "pm".into(),
        Reviewer::CrossVendor => "cross-vendor".into(),
        Reviewer::Profile(key) => key.clone(),
    }
}

/// Shared canonical identity for the effective existing reviewer policy.
pub fn reviewer_policy_digest(policy: &crate::ReviewPolicy) -> Result<String> {
    crate::operating::digest(policy)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoProfileSelection {
    pub version: u32,
    pub id: String,
    pub task: String,
    pub actor: SelectionActor,
    pub selected_at: Timestamp,
    pub role: ProfileRole,
    pub profile: String,
    pub profile_revision: String,
    pub spec_revision: String,
    pub catalog_revision: String,
    pub reason: String,
    pub closest_alternative: Option<String>,
    pub distinction: Option<String>,
    pub review: Option<ProfileReviewSelection>,
}

impl AutoProfileSelection {
    pub fn validate(&self, config: &Config, task: &Task) -> Result<()> {
        ensure!(
            self.version == PROFILE_RECORD_VERSION,
            "unsupported Auto selection version"
        );
        ensure!(
            !self.id.trim().is_empty() && self.task == task.id,
            "wrong Auto selection identity"
        );
        ensure!(
            !self.reason.trim().is_empty(),
            "Auto selection reason is empty"
        );
        ensure!(
            self.spec_revision == crate::plan_contract_revision(task)?,
            "stale task specification"
        );
        ensure!(
            self.catalog_revision == config.profile_catalog_revision()?,
            "stale profile catalog"
        );
        config.check_profile_eligibility(
            &self.profile,
            self.role,
            task.project.as_deref(),
            true,
        )?;
        let revision = config
            .profiles
            .get(&self.profile)
            .and_then(|p| p.metadata.as_ref())
            .map(|m| m.revision.as_str())
            .unwrap_or("legacy");
        ensure!(self.profile_revision == revision, "stale profile revision");
        if let Some(alternative) = &self.closest_alternative {
            ensure!(
                alternative != &self.profile,
                "closest alternative is the selected profile"
            );
            config.check_profile_eligibility(
                alternative,
                self.role,
                task.project.as_deref(),
                true,
            )?;
            ensure!(
                self.distinction
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty()),
                "overlapping profiles require a distinction"
            );
        }
        match self.role {
            ProfileRole::PlanReview | ProfileRole::ImplementationReview => {
                let review = self
                    .review
                    .as_ref()
                    .context("review Auto selection lacks review contract")?;
                ensure!(
                    review.subject.role() == self.role
                        && review.subject.task() == task.id
                        && review.profile == self.profile
                        && review.profile_revision == self.profile_revision,
                    "review Auto selection identity mismatch"
                );
                review.validate_contract(task)?;
            }
            ProfileRole::Implementation | ProfileRole::Planning | ProfileRole::Pm => {
                ensure!(
                    self.review.is_none(),
                    "non-review Auto selection has a review contract"
                );
            }
        }
        Ok(())
    }
}
