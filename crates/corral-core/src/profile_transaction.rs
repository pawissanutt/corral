//! Revision-bound setup application. The journal precedes every active mapping reference.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    Config, Profile, ProfileChoice, ProfileRole, ProfileSetupProposal, ProfileSetupRecord,
    ProfileSetupState, RepairPolicy, Reviewer, SetupBasis, SetupValidationFacets, Store,
};

/// An exact targeted value, including absence before creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSetupChange<T> {
    pub before: T,
    pub after: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSetupDiff {
    pub proposal: String,
    pub proposal_digest: String,
    pub based_on_config_revision: String,
    pub current_config_revision: String,
    pub proposed_config_revision: String,
    pub stale: bool,
    pub profiles: BTreeMap<String, ProfileSetupChange<Option<Profile>>>,
    pub roles: BTreeMap<ProfileRole, ProfileSetupChange<Option<String>>>,
    pub repair: Option<ProfileSetupChange<Option<RepairPolicy>>>,
    pub default_profile: Option<ProfileSetupChange<Option<String>>>,
    pub pm_agent: Option<ProfileSetupChange<String>>,
    pub reviewer: Option<ProfileSetupChange<Option<String>>>,
}

impl Config {
    /// Semantic identity of setup bindings, eligibility and policy, not UI or run limits.
    pub fn profile_setup_revision(&self) -> Result<String> {
        let projects: BTreeMap<_, _> = self
            .projects
            .iter()
            .map(|(name, project)| {
                (
                    name,
                    (
                        &project.path,
                        &project.default_profile,
                        &project.review,
                        &project.dispatch,
                    ),
                )
            })
            .collect();
        crate::operating::digest(&(
            &self.profiles,
            &self.agents,
            &self.harnesses,
            &self.defaults,
            &self.routing,
            &self.pm.agent,
            &self.review,
            projects,
            &self.dispatch,
            &self.automation,
        ))
    }

    /// Apply only approved setup targets, preserving all other configuration.
    pub fn with_profile_setup(&self, proposal: &ProfileSetupProposal) -> Result<Config> {
        proposal.validate(self)?;
        let mut after = self.clone();
        after.profiles.extend(proposal.profiles.clone());
        after.routing.role_profiles.extend(proposal.roles.clone());
        if let Some(policy) = &proposal.repair_policy {
            after.routing.repair = Some(policy.clone());
        }
        for (role, profile) in &proposal.roles {
            match role {
                ProfileRole::Implementation => after.defaults.profile = Some(profile.clone()),
                ProfileRole::Pm => after.pm.agent = profile.clone(),
                ProfileRole::ImplementationReview => {
                    // CrossVendor is itself an independence policy, not just a binding.
                    // Keep that selector rather than silently dropping its implicit requirement.
                    if self.review_policy(None, None).reviewer != Reviewer::CrossVendor {
                        after.review.reviewer = Some(profile.clone());
                    }
                }
                ProfileRole::Planning | ProfileRole::PlanReview => {}
            }
        }
        for (role, profile) in &after.routing.role_profiles {
            after.check_profile_eligibility(profile, *role, None, false)?;
        }
        after.check_setup_families()?;
        Ok(after)
    }

    fn check_setup_families(&self) -> Result<()> {
        let resolve = |name: &str| {
            self.resolve_profile(
                &ProfileChoice {
                    profile: Some(name.into()),
                    ..Default::default()
                },
                None,
                None,
            )
        };
        for project in std::iter::once(None).chain(self.projects.keys().map(|p| Some(p.as_str()))) {
            let policy = self.review_policy(project, None);
            if !policy.cross_family_required() {
                continue;
            }
            let implementer = project
                .and_then(|p| self.projects.get(p))
                .and_then(|p| p.default_profile.as_deref())
                .or(self.defaults.profile.as_deref())
                .unwrap_or(crate::FALLBACK_PROFILE);
            let mut authors = vec![implementer];
            if let Some(planner) = self.routing.role_profiles.get(&ProfileRole::Planning) {
                authors.push(planner);
            }
            for rule in &self.routing.rules {
                if project.is_none()
                    || rule.project.is_empty()
                    || project.is_some_and(|p| rule.project.iter().any(|name| name == p))
                {
                    authors.push(&rule.profile);
                }
            }
            for author in authors {
                let author = resolve(author)?;
                let reviewer = match &policy.reviewer {
                    Reviewer::None | Reviewer::Auto => None,
                    Reviewer::Pm => Some(self.pm.agent.clone()),
                    Reviewer::Profile(name) => Some(name.clone()),
                    Reviewer::CrossVendor => Some(
                        self.cross_vendor_reviewer(
                            author
                                .vendor
                                .as_deref()
                                .context("unknown setup author vendor")?,
                        )?,
                    ),
                };
                let automatic = self
                    .routing
                    .review_profiles
                    .values()
                    .filter(|_| policy.reviewer == Reviewer::Auto);
                for name in reviewer
                    .iter()
                    .chain(automatic)
                    .chain(self.routing.role_profiles.get(&ProfileRole::PlanReview))
                    .chain(
                        self.routing
                            .role_profiles
                            .get(&ProfileRole::ImplementationReview),
                    )
                {
                    let reviewer = resolve(name)?;
                    policy.check_reviewer_family(
                        author.vendor.as_deref(),
                        reviewer.vendor.as_deref(),
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Versioned atomic recovery document. Completion is proved by the append-only journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileSetupIntent {
    version: u32,
    record: ProfileSetupRecord,
    proposal_digest: String,
    before_revision: String,
    after_revision: String,
    validation: SetupValidationFacets,
}

impl Store {
    pub fn profile_setup_diff(&self) -> Result<ProfileSetupDiff> {
        let record = self.profile_setup()?;
        let proposal = record.proposal.as_ref().context("no setup proposal")?;
        let before = self.config_layers()?.config()?;
        let after = before.with_profile_setup(proposal)?;
        let current_config_revision = before.profile_setup_revision()?;
        Ok(ProfileSetupDiff {
            proposal: proposal.id.clone(),
            proposal_digest: proposal.digest()?,
            based_on_config_revision: proposal.based_on_config_revision.clone(),
            stale: proposal.based_on_config_revision != current_config_revision,
            current_config_revision,
            proposed_config_revision: after.profile_setup_revision()?,
            profiles: proposal
                .profiles
                .iter()
                .map(|(name, profile)| {
                    (
                        name.clone(),
                        ProfileSetupChange {
                            before: before.profiles.get(name).cloned(),
                            after: Some(profile.clone()),
                        },
                    )
                })
                .collect(),
            roles: proposal
                .roles
                .iter()
                .map(|(role, profile)| {
                    (
                        *role,
                        ProfileSetupChange {
                            before: before.routing.role_profiles.get(role).cloned(),
                            after: Some(profile.clone()),
                        },
                    )
                })
                .collect(),
            repair: proposal
                .repair_policy
                .as_ref()
                .map(|policy| ProfileSetupChange {
                    before: before.routing.repair.clone(),
                    after: Some(policy.clone()),
                }),
            default_profile: changed(&before.defaults.profile, &after.defaults.profile),
            pm_agent: changed(&before.pm.agent, &after.pm.agent),
            reviewer: changed(&before.review.reviewer, &after.review.reviewer),
        })
    }

    /// Validate and commit under the records/config boundaries, or resume an exact intent.
    pub fn apply_profile_setup(
        &self,
        validate: impl Fn(&ProfileSetupProposal, &Config) -> Result<SetupValidationFacets>,
    ) -> Result<ProfileSetupRecord> {
        self.check_runtime()?;
        let _records = self.lock("profile-records")?;
        let record = self.profile_setup()?;
        if record.state == ProfileSetupState::Applied {
            return Ok(record);
        }
        ensure!(
            record.state == ProfileSetupState::Proposed,
            "setup is not proposed"
        );
        let proposal = record.proposal.as_ref().context("no setup proposal")?;
        let approval = record
            .approval
            .as_ref()
            .context("setup requires persisted approval")?;
        proposal.check_approval(approval)?;
        if let Some(applied) = self.profile_setup_history()?.into_iter().find(|previous| {
            previous.state == ProfileSetupState::Applied
                && previous
                    .proposal
                    .as_ref()
                    .is_some_and(|old| old.id == proposal.id)
        }) {
            return Ok(applied);
        }
        ensure!(
            !matches!(proposal.basis, SetupBasis::Manual),
            "manual setup uses config CLI"
        );
        let pending = self.pending_profile_setup_intent()?;
        // Refuse already-stale input without edit_config observing a raw file edit.
        // The check inside that method's lock remains the decisive concurrency boundary.
        let revision = self.config_layers()?.config()?.profile_setup_revision()?;
        match &pending {
            Some(saved) => ensure!(
                revision == saved.before_revision || revision == saved.after_revision,
                "relevant configuration changed during pending setup"
            ),
            None => ensure!(
                revision == proposal.based_on_config_revision,
                "stale setup proposal: relevant configuration changed"
            ),
        }
        let mut intent = None;
        let config = self.edit_config(|doc| {
            // This read and all revision checks execute under edit_config's config lock.
            let before = self.config_layers()?.config()?;
            let revision = before.profile_setup_revision()?;
            proposal.validate(&before)?;
            let after = before.with_profile_setup(proposal)?;
            after.validate(self.root())?;
            let after_revision = after.profile_setup_revision()?;
            let saved = match &pending {
                Some(saved) => {
                    ensure!(
                        saved.record == record && saved.proposal_digest == proposal.digest()?,
                        "pending setup intent differs from approved proposal"
                    );
                    ensure!(
                        revision == saved.before_revision || revision == saved.after_revision,
                        "relevant configuration changed during pending setup"
                    );
                    ensure!(
                        after_revision == saved.after_revision,
                        "setup recovery target changed"
                    );
                    saved.clone()
                }
                None => {
                    ensure!(
                        revision == proposal.based_on_config_revision,
                        "stale setup proposal: relevant configuration changed"
                    );
                    ProfileSetupIntent {
                        version: crate::PROFILE_RECORD_VERSION,
                        record: record.clone(),
                        proposal_digest: proposal.digest()?,
                        before_revision: revision,
                        after_revision,
                        validation: SetupValidationFacets::default(),
                    }
                }
            };
            let facets = validate(proposal, &before)?;
            ensure!(
                facets.configuration_valid,
                "setup configuration validation failed"
            );
            let saved = ProfileSetupIntent {
                validation: facets,
                ..saved
            };
            doc.apply_config_changes(&before, &after)?;
            // Reject unrepresentable inherited-field removals before publishing intent.
            let mut layers = self.config_layers()?;
            layers.replace(crate::ConfigLayer::Workspace, doc)?;
            ensure!(
                layers.config()?.profile_setup_revision()? == saved.after_revision,
                "setup delta does not produce the approved effective bindings"
            );
            crate::profile_records::write_journal(
                &self.state_dir().join("profile-setup-intent.json"),
                &saved,
            )?;
            intent = Some(saved);
            Ok(())
        })?;
        let intent = intent.context("missing setup apply intent")?;
        ensure!(
            config.profile_setup_revision()? == intent.after_revision,
            "setup config was not applied"
        );
        let applied = ProfileSetupRecord {
            revision: record
                .revision
                .checked_add(1)
                .context("setup revision exhausted")?,
            state: ProfileSetupState::Applied,
            applied_config_revision: Some(intent.after_revision),
            validation: intent.validation,
            ..record
        };
        self.append_profile_setup_locked(&applied)?;
        Ok(applied)
    }

    pub(crate) fn pending_profile_setup_intent(&self) -> Result<Option<ProfileSetupIntent>> {
        let intent: Option<ProfileSetupIntent> = crate::profile_records::read_journal(
            &self.state_dir().join("profile-setup-intent.json"),
        )?;
        let Some(intent) = intent else {
            return Ok(None);
        };
        ensure!(
            intent.version == crate::PROFILE_RECORD_VERSION,
            "unsupported setup intent"
        );
        intent.record.validate_integrity()?;
        let proposal = intent
            .record
            .proposal
            .as_ref()
            .context("intent lacks proposal")?;
        proposal.check_approval(
            intent
                .record
                .approval
                .as_ref()
                .context("intent lacks approval")?,
        )?;
        ensure!(
            intent.record.state == ProfileSetupState::Proposed
                && intent.proposal_digest == proposal.digest()?
                && intent.before_revision == proposal.based_on_config_revision
                && intent.validation.configuration_valid,
            "invalid setup intent"
        );
        let completed = self.profile_setup_history()?.iter().any(|record| {
            record.state == ProfileSetupState::Applied
                && record.proposal == intent.record.proposal
                && record.approval == intent.record.approval
                && record.applied_config_revision.as_ref() == Some(&intent.after_revision)
        });
        Ok((!completed).then_some(intent))
    }
}

fn changed<T: Clone + PartialEq>(before: &T, after: &T) -> Option<ProfileSetupChange<T>> {
    (before != after).then(|| ProfileSetupChange {
        before: before.clone(),
        after: after.clone(),
    })
}
