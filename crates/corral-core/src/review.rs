//! Review policy: the `[review]` table, overridable per project (`[projects.<p>.review]`) and per task
//! (frontmatter `review: owner|auto`). The default policy is the manual flow: no gate, no reviewer, the owner
//! accepts.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config_validation::SemanticInputs;
use crate::{Config, SessionPolicy, Task};

/// The existing planning contract identity, kept independent from execution progress.
pub fn plan_contract_revision(task: &Task) -> Result<String> {
    crate::operating::digest(&(
        crate::operating::digest(&(&task.id, &task.title, &task.project, &task.body))?,
        &task.completion,
        &task.depends_on,
    ))
}

/// Who accepts a run that reached Review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accept {
    /// Stop at Review for the owner.
    #[default]
    Owner,
    /// Land automatically when the gate and the review pass.
    Auto,
}

#[cfg(test)]
mod plan_accept_tests {
    use super::*;

    #[test]
    fn plan_accept_defaults_to_owner_and_inherits_independently() {
        let mut config = Config::default_for(std::path::Path::new("."));
        assert_eq!(
            config.review_policy(None, Some(Accept::Auto)).plan_accept,
            Accept::Owner
        );
        config.review = toml::from_str("plan_accept = 'auto'").unwrap();
        assert_eq!(
            config.review_policy(None, Some(Accept::Owner)).plan_accept,
            Accept::Auto
        );
        assert_eq!(
            config.review_policy_sources(None, None).plan_accept,
            PolicySource::Global
        );
    }
}

/// How an accepted run lands on its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    #[default]
    Rebase,
    Ff,
}

/// Who reviews a run after its gate passes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub enum Reviewer {
    #[default]
    None,
    /// The workspace PM agent.
    Pm,
    /// An independent reviewer selected from a different vendor.
    CrossVendor,
    /// Explicit opt-in: the profile mapped to the task's review contract in `routing.review_profiles`.
    Auto,
    /// An independent agent launched with this profile.
    Profile(String),
}

impl Reviewer {
    /// "none", "pm", "cross-vendor" and "auto" are keywords; anything else names a profile.
    pub fn parse(s: &str) -> Reviewer {
        match s.trim() {
            "none" => Reviewer::None,
            "pm" => Reviewer::Pm,
            "cross-vendor" => Reviewer::CrossVendor,
            "auto" => Reviewer::Auto,
            other => Reviewer::Profile(other.to_string()),
        }
    }
}

/// One layer of review settings (global `[review]` or `[projects.<p>.review]`); unset keys inherit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_session: Option<SessionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_session: Option<SessionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_accept: Option<Accept>,
    /// Shell commands run in the run worktree, in order; empty = no gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Vec<String>>,
    /// Gate environment overrides, merged by key across policy layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// Maximum source lines per extension (without a dot), merged by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_limits: Option<BTreeMap<String, usize>>,
    /// Named gate tiers (name -> commands), merged by key; `full` is reserved for `gate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_tiers: Option<BTreeMap<String, Vec<String>>>,
    /// Default tiers for Plan members and main-bound review rounds, merged field by field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_policy: Option<crate::TierPolicy>,
    /// Operational Plan-target checkpoint interval; never part of [`ReviewPolicy`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_checkpoint_every: Option<u32>,
    /// Operational infrastructure-wait bound; never part of [`ReviewPolicy`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infra_wait_secs: Option<u64>,
    /// "none" | "pm" | "cross-vendor" | "auto" | "<profile>".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    /// Globs whose changes force the high-consequence review contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_paths: Option<Vec<String>>,
    /// Require a reviewer from a different model vendor/family. Unset: off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_cross_family: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Accept>,
    /// Automatic request-changes loops before escalating to Needs me.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_worktree: Option<bool>,
    /// Deprecated (T-214): accepted so existing configs load, but ignored. Diff size no longer
    /// bypasses review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_diff_lines: Option<u64>,
    /// Globs (relative to the repo root); touching any forces owner acceptance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_paths: Option<Vec<String>>,
    /// Kill switch for the review autopilot (workspace-wide, `[review]` only). Unset: on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autopilot: Option<bool>,
}

impl ReviewOverride {
    /// True when no effective review setting is present; the deprecated, ignored `max_diff_lines`
    /// and operational checkpoint/infra-wait bounds do not count, so they cannot activate
    /// review-policy requirements.
    pub fn is_empty(&self) -> bool {
        let mut effective = self.clone();
        effective.max_diff_lines = None;
        effective.plan_checkpoint_every = None;
        effective.infra_wait_secs = None;
        effective == ReviewOverride::default()
    }

    fn apply(&self, p: &mut ReviewPolicy) {
        if let Some(v) = self.worker_session {
            p.worker_session = v;
        }
        if let Some(v) = self.reviewer_session {
            p.reviewer_session = v;
        }
        if let Some(v) = self.plan_required {
            p.plan_required = v;
        }
        if let Some(v) = self.plan_accept {
            p.plan_accept = v;
        }
        if let Some(v) = &self.gate {
            p.gate = v.clone();
        }
        if let Some(v) = &self.env {
            p.env.extend(v.clone());
        }
        if let Some(v) = &self.size_limits {
            p.size_limits.extend(v.clone());
        }
        if let Some(v) = &self.gate_tiers {
            p.gate_tiers.extend(v.clone());
        }
        if let Some(v) = &self.tier_policy {
            p.tier_policy.merge(v);
        }
        if let Some(v) = &self.reviewer {
            p.reviewer = Reviewer::parse(v);
        }
        if let Some(v) = self.require_cross_family {
            p.require_cross_family = v;
        }
        if let Some(v) = self.accept {
            p.accept = v;
        }
        if let Some(v) = self.max_rounds {
            p.max_rounds = v;
        }
        if let Some(v) = self.merge {
            p.merge = v;
        }
        if let Some(v) = self.remove_worktree {
            p.remove_worktree = v;
        }
        if let Some(v) = &self.owner_paths {
            p.owner_paths = v.clone();
        }
        if let Some(v) = &self.risk_paths {
            p.risk_paths = v.clone();
        }
    }
}

/// The effective review policy for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewPolicy {
    /// Omitted at the legacy default so existing policy identities remain stable.
    #[serde(skip_serializing_if = "SessionPolicy::is_reuse")]
    pub worker_session: SessionPolicy,
    #[serde(skip_serializing_if = "SessionPolicy::is_reuse")]
    pub reviewer_session: SessionPolicy,
    pub plan_required: bool,
    pub plan_accept: Accept,
    pub gate: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub size_limits: BTreeMap<String, usize>,
    pub reviewer: Reviewer,
    pub require_cross_family: bool,
    pub accept: Accept,
    pub max_rounds: u32,
    pub merge: MergeMode,
    pub remove_worktree: bool,
    pub owner_paths: Vec<String>,
    /// Omitted when empty so existing policy digests are unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub risk_paths: Vec<String>,
    /// Omitted when empty so untiered policy digests are unchanged.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub gate_tiers: BTreeMap<String, Vec<String>>,
    #[serde(skip_serializing_if = "crate::TierPolicy::is_empty")]
    pub tier_policy: crate::TierPolicy,
}

impl Default for ReviewPolicy {
    /// Today's manual flow: no gate, no reviewer, the owner accepts.
    fn default() -> Self {
        ReviewPolicy {
            worker_session: SessionPolicy::Reuse,
            reviewer_session: SessionPolicy::Reuse,
            plan_required: false,
            plan_accept: Accept::Owner,
            gate: Vec::new(),
            env: BTreeMap::new(),
            size_limits: BTreeMap::from([("rs".into(), 800)]),
            reviewer: Reviewer::None,
            require_cross_family: false,
            accept: Accept::Owner,
            max_rounds: 2,
            merge: MergeMode::Rebase,
            remove_worktree: true,
            owner_paths: Vec::new(),
            risk_paths: Vec::new(),
            gate_tiers: BTreeMap::new(),
            tier_policy: crate::TierPolicy::default(),
        }
    }
}

impl ReviewPolicy {
    /// The cross-vendor selector is itself an explicit opt-in to reviewer diversity.
    pub fn cross_family_required(&self) -> bool {
        self.require_cross_family || self.reviewer == Reviewer::CrossVendor
    }

    /// Checks the selected reviewer's vendor only when diversity is required.
    pub fn check_reviewer_family(
        &self,
        author_vendor: Option<&str>,
        reviewer_vendor: Option<&str>,
    ) -> Result<()> {
        if !self.cross_family_required() {
            return Ok(());
        }
        let author = author_vendor
            .filter(|v| !v.trim().is_empty())
            .context("cross-family review is required but author vendor is unknown")?;
        let reviewer = reviewer_vendor
            .filter(|v| !v.trim().is_empty())
            .context("cross-family review is required but reviewer vendor is unknown")?;
        if author == reviewer {
            bail!(
                "cross-family review is required: reviewer vendor '{reviewer}' matches author vendor '{author}'"
            );
        }
        Ok(())
    }
}

/// Which layer an effective review-policy value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    /// The built-in default (nothing set it).
    #[default]
    Default,
    /// `[review]` in corral.toml.
    Global,
    /// `[projects.<p>.review]` in corral.toml.
    Project,
    /// The task's `review:` frontmatter.
    Task,
}

impl PolicySource {
    /// "built-in default", "[review]", "[projects.<project>.review]" or "task review:".
    pub fn label(self, project: Option<&str>) -> String {
        match self {
            PolicySource::Default => "built-in default".into(),
            PolicySource::Global => "[review]".into(),
            PolicySource::Project => format!("[projects.{}.review]", project.unwrap_or("?")),
            PolicySource::Task => "task review:".into(),
        }
    }
}

/// Where each field of an effective [`ReviewPolicy`] came from (see [`Config::review_policy_sources`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PolicySources {
    pub worker_session: PolicySource,
    pub reviewer_session: PolicySource,
    pub plan_required: PolicySource,
    pub plan_accept: PolicySource,
    pub gate: PolicySource,
    /// Highest layer contributing a key; maps merge rather than replace.
    pub env: PolicySource,
    /// Highest layer contributing a key; maps merge rather than replace.
    pub size_limits: PolicySource,
    pub reviewer: PolicySource,
    pub require_cross_family: PolicySource,
    pub accept: PolicySource,
    pub max_rounds: PolicySource,
    pub merge: PolicySource,
    pub remove_worktree: PolicySource,
    pub owner_paths: PolicySource,
    /// The workspace-wide kill switch: `[review]` or the default (on).
    pub autopilot: PolicySource,
    /// Highest layer contributing a tier; tiers merge by name rather than replace.
    pub gate_tiers: PolicySource,
    pub tier_policy_plan_member: PolicySource,
    pub tier_policy_review_rounds: PolicySource,
    /// Operational, read through [`Config::plan_checkpoint_every`]; not part of the policy.
    pub plan_checkpoint_every: PolicySource,
}

/// Checks a glob in the syntax corral matches: `/`-separated relative segments, `*` within a segment, `**` as
/// a whole segment for any number of directories. Character classes and braces are not supported.
pub fn validate_glob(glob: &str) -> Result<()> {
    if glob.trim().is_empty() {
        bail!("empty glob");
    }
    if glob.starts_with('/') {
        bail!("glob {glob:?} must be relative");
    }
    if glob.contains(['[', ']', '{', '}']) {
        bail!("glob {glob:?}: character classes and braces are not supported");
    }
    for seg in glob.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            bail!("glob {glob:?} has an empty, '.' or '..' path segment");
        }
        if seg.contains("**") && seg != "**" {
            bail!("glob {glob:?}: '**' must be a whole path segment");
        }
    }
    Ok(())
}

impl Config {
    /// An explicit owner restriction at any layer is never replaced by a scope grant.
    /// Built-in Owner defaults are not explicit exceptions.
    pub fn explicit_owner_acceptance(&self, task: &Task, plan: bool) -> bool {
        let project = task.project.as_ref().and_then(|key| self.projects.get(key));
        if plan {
            self.review.plan_accept == Some(Accept::Owner)
                || project.is_some_and(|project| project.review.plan_accept == Some(Accept::Owner))
        } else {
            task.review == Some(Accept::Owner)
                || self.review.accept == Some(Accept::Owner)
                || project.is_some_and(|project| project.review.accept == Some(Accept::Owner))
        }
    }

    /// Selects an independent reviewer by the reviewer's vendor, without a same-vendor fallback.
    pub fn cross_vendor_reviewer(&self, implementer_vendor: &str) -> Result<String> {
        if implementer_vendor.trim().is_empty() {
            bail!("cross-vendor review requires a known implementer vendor");
        }
        let vendor = match implementer_vendor {
            "anthropic" => "openai",
            "openai" => "anthropic",
            other => {
                let mut vendors = self
                    .routing
                    .reviewer_by_vendor
                    .keys()
                    .filter(|vendor| vendor.as_str() != other);
                let vendor = vendors
                    .next()
                    .context("no different reviewer vendor configured")?;
                if vendors.next().is_some() {
                    bail!("ambiguous reviewer vendors for implementer vendor '{other}'");
                }
                vendor.as_str()
            }
        };
        self.reviewer_profile_for_vendor(vendor)
    }

    pub(crate) fn reviewer_profile_for_vendor(&self, vendor: &str) -> Result<String> {
        if vendor.trim().is_empty() {
            bail!("reviewer mapping requires a known vendor");
        }
        let name = self
            .routing
            .reviewer_by_vendor
            .get(vendor)
            .with_context(|| format!("no reviewer configured for vendor '{vendor}'"))?;
        let resolved = self.resolve_profile(
            &crate::ProfileChoice {
                profile: Some(name.clone()),
                ..Default::default()
            },
            None,
            None,
        )?;
        let actual = resolved
            .vendor
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("reviewer profile '{name}' has unknown vendor"))?;
        if actual != vendor {
            bail!(
                "reviewer profile '{name}' vendor '{actual}' does not match mapping vendor '{vendor}'"
            );
        }
        Ok(name.clone())
    }

    /// The configuration as policy and consent identities see it: operational review keys removed,
    /// borrowed unchanged when none is set.
    pub(crate) fn policy_view(&self) -> std::borrow::Cow<'_, Config> {
        let operational = std::iter::once(&self.review)
            .chain(self.projects.values().map(|p| &p.review))
            .any(|layer| layer.plan_checkpoint_every.is_some());
        if !operational {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut config = self.clone();
        config.review.plan_checkpoint_every = None;
        for project in config.projects.values_mut() {
            project.review.plan_checkpoint_every = None;
        }
        std::borrow::Cow::Owned(config)
    }

    /// `plan_checkpoint_every`, project over `[review]`. Operational: it never enters [`ReviewPolicy`]
    /// or a policy revision, so changing it stales no receipt.
    pub fn plan_checkpoint_every(&self, project: Option<&str>) -> Option<u32> {
        project
            .and_then(|p| self.projects.get(p))
            .and_then(|p| p.review.plan_checkpoint_every)
            .or(self.review.plan_checkpoint_every)
    }

    /// The `[review] autopilot` kill switch (default on). With the manual default policy the autopilot has
    /// nothing to do either way.
    pub fn autopilot_enabled(&self) -> bool {
        self.review.autopilot.unwrap_or(true)
    }

    /// Layers (`[review]`, `[projects.<p>.review]`) that still set the deprecated, ignored `max_diff_lines`.
    pub fn deprecated_max_diff_lines_layers(&self) -> Vec<String> {
        std::iter::once(("[review]".to_string(), &self.review))
            .chain(
                self.projects
                    .iter()
                    .map(|(k, p)| (format!("[projects.{k}.review]"), &p.review)),
            )
            .filter(|(_, layer)| layer.max_diff_lines.is_some())
            .map(|(name, _)| name)
            .collect()
    }

    /// Whether review was explicitly configured, even if its effective gate is empty.
    pub fn has_review_policy(&self, project: Option<&str>, task_accept: Option<Accept>) -> bool {
        !self.review.is_empty()
            || project
                .and_then(|name| self.projects.get(name))
                .is_some_and(|project| !project.review.is_empty())
            || task_accept.is_some()
    }

    /// Whether a plan is required, with a task override above project and global settings.
    pub fn plan_required(&self, project: Option<&str>, task_override: Option<bool>) -> bool {
        task_override.unwrap_or_else(|| self.review_policy(project, None).plan_required)
    }

    /// Effective policy: built-in defaults < `[review]` < `[projects.<project>.review]` < task `review:`.
    pub fn review_policy(
        &self,
        project: Option<&str>,
        task_accept: Option<Accept>,
    ) -> ReviewPolicy {
        let mut policy = ReviewPolicy::default();
        self.review.apply(&mut policy);
        if let Some(p) = project.and_then(|p| self.projects.get(p)) {
            p.review.apply(&mut policy);
        }
        if let Some(a) = task_accept {
            policy.accept = a;
        }
        policy
    }

    /// Effective task policy, including its independent reviewer override and reviewer rules.
    pub fn task_review_policy(&self, task: &Task) -> ReviewPolicy {
        let mut policy = self.review_policy(task.project.as_deref(), task.review);
        if let Some(reviewer) = &task.reviewer {
            policy.reviewer = Reviewer::parse(reviewer);
        } else if let Some((_, rule)) = self.reviewer_rule(task) {
            // A replaced workspace `cross-vendor` selector still asked for reviewer diversity.
            if policy.reviewer == Reviewer::CrossVendor {
                policy.require_cross_family = true;
            }
            policy.reviewer = Reviewer::parse(&rule.profile);
        }
        policy
    }

    /// The first matching `[[routing.reviewer_rules]]` entry (one-based index) when it decides the
    /// reviewer: a task `reviewer:` or a `[projects.<p>.review] reviewer` keeps precedence.
    /// Rules with an invalid regex never match; config validation reports them.
    pub fn reviewer_rule(&self, task: &Task) -> Option<(usize, &crate::RoutingRule)> {
        let project_reviewer = task
            .project
            .as_deref()
            .and_then(|p| self.projects.get(p))
            .is_some_and(|p| p.review.reviewer.is_some());
        if task.reviewer.is_some() || project_reviewer {
            return None;
        }
        self.routing
            .reviewer_rules
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.matches(task).unwrap_or(false))
            .map(|(index, rule)| (index + 1, rule))
    }

    /// Which layer set each value of [`Config::review_policy`] for the same arguments: the last layer that sets a
    /// key wins, as in the policy itself.
    pub fn review_policy_sources(
        &self,
        project: Option<&str>,
        task_accept: Option<Accept>,
    ) -> PolicySources {
        let proj = project
            .and_then(|p| self.projects.get(p))
            .map(|p| &p.review);
        let src = |set: fn(&ReviewOverride) -> bool| {
            if proj.is_some_and(set) {
                PolicySource::Project
            } else if set(&self.review) {
                PolicySource::Global
            } else {
                PolicySource::Default
            }
        };
        PolicySources {
            worker_session: src(|r| r.worker_session.is_some()),
            reviewer_session: src(|r| r.reviewer_session.is_some()),
            plan_required: src(|r| r.plan_required.is_some()),
            plan_accept: src(|r| r.plan_accept.is_some()),
            gate: src(|r| r.gate.is_some()),
            env: src(|r| r.env.as_ref().is_some_and(|m| !m.is_empty())),
            size_limits: src(|r| r.size_limits.as_ref().is_some_and(|m| !m.is_empty())),
            reviewer: src(|r| r.reviewer.is_some()),
            require_cross_family: src(|r| r.require_cross_family.is_some()),
            accept: if task_accept.is_some() {
                PolicySource::Task
            } else {
                src(|r| r.accept.is_some())
            },
            max_rounds: src(|r| r.max_rounds.is_some()),
            merge: src(|r| r.merge.is_some()),
            remove_worktree: src(|r| r.remove_worktree.is_some()),
            owner_paths: src(|r| r.owner_paths.is_some()),
            autopilot: if self.review.autopilot.is_some() {
                PolicySource::Global
            } else {
                PolicySource::Default
            },
            gate_tiers: src(|r| r.gate_tiers.as_ref().is_some_and(|m| !m.is_empty())),
            tier_policy_plan_member: src(|r| {
                r.tier_policy
                    .as_ref()
                    .is_some_and(|t| t.plan_member.is_some())
            }),
            tier_policy_review_rounds: src(|r| {
                r.tier_policy
                    .as_ref()
                    .is_some_and(|t| t.review_rounds.is_some())
            }),
            plan_checkpoint_every: src(|r| r.plan_checkpoint_every.is_some()),
        }
    }

    /// Checks every review layer: reviewers must be "none", "pm" or a configured profile, `owner_paths` must be
    /// valid globs, gate commands must be non-empty. Reports every problem in one error.
    pub fn validate_review(&self) -> Result<()> {
        let problems = self.review_problems();
        if !problems.is_empty() {
            bail!("invalid review config: {}", problems.join("; "));
        }
        Ok(())
    }

    /// The problems `validate_review` reports, one per entry.
    pub fn review_problems(&self) -> Vec<String> {
        self.review_problems_with_inputs(&SemanticInputs::default())
    }

    pub(crate) fn review_problems_with_inputs(&self, inputs: &SemanticInputs) -> Vec<String> {
        let mut profiles = self.profile_names();
        profiles.extend_from_slice(&inputs.rejected_profiles);
        let mut problems = Vec::new();
        for (vendor, profile) in &self.routing.reviewer_by_vendor {
            // Vendor resolution needs the complete profile and harness inventories.
            // A blank vendor remains independently invalid without either inventory.
            if !vendor.trim().is_empty() && !inputs.reviewer_available(self, profile) {
                continue;
            }
            if let Err(error) = self.reviewer_profile_for_vendor(vendor) {
                problems.push(format!("routing.reviewer_by_vendor.{vendor}: {error:#}"));
            }
        }
        for (index, rule) in self.routing.reviewer_rules.iter().enumerate() {
            let name = format!("routing.reviewer_rules[{}]", index + 1);
            if let Reviewer::Profile(p) = Reviewer::parse(&rule.profile)
                && inputs.profiles_complete()
                && !profiles.contains(&p)
            {
                problems.push(format!(
                    "{name}: unknown reviewer profile '{p}' (use none, pm, cross-vendor or one of: {})",
                    profiles.join(", ")
                ));
            }
            for regex in [&rule.title_regex, &rule.body_regex].into_iter().flatten() {
                if let Err(error) = regex::Regex::new(regex) {
                    problems.push(format!("{name}: invalid regex: {error}"));
                }
            }
        }
        let layers = std::iter::once(("[review]".to_string(), &self.review)).chain(
            self.projects
                .iter()
                .map(|(k, p)| (format!("[projects.{k}.review]"), &p.review)),
        );
        for (name, layer) in layers {
            if let Some(Reviewer::Profile(p)) = layer.reviewer.as_deref().map(Reviewer::parse)
                && inputs.profiles_complete()
                && !profiles.contains(&p)
            {
                problems.push(format!(
                    "{name}: unknown reviewer profile '{p}' (use none, pm, cross-vendor, auto or one of: {})",
                    profiles.join(", ")
                ));
            }
            for glob in layer.owner_paths.iter().flatten() {
                if let Err(e) = validate_glob(glob) {
                    problems.push(format!("{name}: owner_paths: {e}"));
                }
            }
            for glob in layer.risk_paths.iter().flatten() {
                if let Err(e) = validate_glob(glob) {
                    problems.push(format!("{name}: risk_paths: {e}"));
                }
            }
            if layer.gate.iter().flatten().any(|c| c.trim().is_empty()) {
                problems.push(format!("{name}: gate has an empty command"));
            }
            for (key, value) in layer.env.iter().flatten() {
                let mut chars = key.bytes();
                if !chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                    || !chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    problems.push(format!(
                        "{name}: env key {key:?} must be a portable shell name"
                    ));
                }
                if key.starts_with("CORRAL_GATE_")
                    || key.starts_with("CORRAL_CANDIDATE_")
                    || key == "CORRAL_INTEGRATION_BASE"
                    || key == "CORRAL_SIZE_LIMITS"
                {
                    problems.push(format!(
                        "{name}: env key {key:?} is reserved for gate identity"
                    ));
                }
                if value.contains('\0') {
                    problems.push(format!("{name}: env value for {key:?} contains NUL"));
                }
            }
            for (extension, limit) in layer.size_limits.iter().flatten() {
                if extension.is_empty() || !extension.bytes().all(|c| c.is_ascii_alphanumeric()) {
                    problems.push(format!(
                        "{name}: size_limits key {extension:?} must be an alphanumeric extension without a dot"
                    ));
                }
                if *limit == 0 {
                    problems.push(format!("{name}: size_limits.{extension} must be positive"));
                }
            }
        }
        problems.extend(self.tier_problems(inputs));
        problems.extend(self.review_profile_problems());
        for (k, p) in &self.projects {
            if p.review.autopilot.is_some() {
                problems.push(format!(
                    "[projects.{k}.review]: autopilot is workspace-wide; set it under [review]"
                ));
            }
        }
        problems
    }
}
