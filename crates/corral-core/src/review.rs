//! Review policy: the `[review]` table, overridable per project (`[projects.<p>.review]`) and per task
//! (frontmatter `review: owner|auto`). The default policy is the manual flow: no gate, no reviewer, the owner
//! accepts.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::Config;

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
    /// An independent agent launched with this profile.
    Profile(String),
}

impl Reviewer {
    /// "none", "pm" and "cross-vendor" are keywords; anything else names a profile.
    pub fn parse(s: &str) -> Reviewer {
        match s.trim() {
            "none" => Reviewer::None,
            "pm" => Reviewer::Pm,
            "cross-vendor" => Reviewer::CrossVendor,
            other => Reviewer::Profile(other.to_string()),
        }
    }
}

/// One layer of review settings (global `[review]` or `[projects.<p>.review]`); unset keys inherit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_required: Option<bool>,
    /// Shell commands run in the run worktree, in order; empty = no gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Vec<String>>,
    /// Gate environment overrides, merged by key across policy layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// Maximum source lines per extension (without a dot), merged by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_limits: Option<BTreeMap<String, usize>>,
    /// "none" | "pm" | "cross-vendor" | "<profile>".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<Accept>,
    /// Automatic request-changes loops before escalating to Needs me.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_worktree: Option<bool>,
    /// Bigger diffs always need the owner.
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
    pub fn is_empty(&self) -> bool {
        *self == ReviewOverride::default()
    }

    fn apply(&self, p: &mut ReviewPolicy) {
        if let Some(v) = self.plan_required {
            p.plan_required = v;
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
        if let Some(v) = &self.reviewer {
            p.reviewer = Reviewer::parse(v);
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
        if let Some(v) = self.max_diff_lines {
            p.max_diff_lines = v;
        }
        if let Some(v) = &self.owner_paths {
            p.owner_paths = v.clone();
        }
    }
}

/// The effective review policy for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewPolicy {
    pub plan_required: bool,
    pub gate: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub size_limits: BTreeMap<String, usize>,
    pub reviewer: Reviewer,
    pub accept: Accept,
    pub max_rounds: u32,
    pub merge: MergeMode,
    pub remove_worktree: bool,
    pub max_diff_lines: u64,
    pub owner_paths: Vec<String>,
}

impl Default for ReviewPolicy {
    /// Today's manual flow: no gate, no reviewer, the owner accepts.
    fn default() -> Self {
        ReviewPolicy {
            plan_required: false,
            gate: Vec::new(),
            env: BTreeMap::new(),
            size_limits: BTreeMap::from([("rs".into(), 800)]),
            reviewer: Reviewer::None,
            accept: Accept::Owner,
            max_rounds: 2,
            merge: MergeMode::Rebase,
            remove_worktree: true,
            max_diff_lines: 1500,
            owner_paths: Vec::new(),
        }
    }
}

/// Which layer an effective review-policy value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
    pub plan_required: PolicySource,
    pub gate: PolicySource,
    /// Highest layer contributing a key; maps merge rather than replace.
    pub env: PolicySource,
    /// Highest layer contributing a key; maps merge rather than replace.
    pub size_limits: PolicySource,
    pub reviewer: PolicySource,
    pub accept: PolicySource,
    pub max_rounds: PolicySource,
    pub merge: PolicySource,
    pub remove_worktree: PolicySource,
    pub max_diff_lines: PolicySource,
    pub owner_paths: PolicySource,
    /// The workspace-wide kill switch: `[review]` or the default (on).
    pub autopilot: PolicySource,
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

    /// The `[review] autopilot` kill switch (default on). With the manual default policy the autopilot has
    /// nothing to do either way.
    pub fn autopilot_enabled(&self) -> bool {
        self.review.autopilot.unwrap_or(true)
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
            plan_required: src(|r| r.plan_required.is_some()),
            gate: src(|r| r.gate.is_some()),
            env: src(|r| r.env.as_ref().is_some_and(|m| !m.is_empty())),
            size_limits: src(|r| r.size_limits.as_ref().is_some_and(|m| !m.is_empty())),
            reviewer: src(|r| r.reviewer.is_some()),
            accept: if task_accept.is_some() {
                PolicySource::Task
            } else {
                src(|r| r.accept.is_some())
            },
            max_rounds: src(|r| r.max_rounds.is_some()),
            merge: src(|r| r.merge.is_some()),
            remove_worktree: src(|r| r.remove_worktree.is_some()),
            max_diff_lines: src(|r| r.max_diff_lines.is_some()),
            owner_paths: src(|r| r.owner_paths.is_some()),
            autopilot: if self.review.autopilot.is_some() {
                PolicySource::Global
            } else {
                PolicySource::Default
            },
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
        let profiles = self.profile_names();
        let mut problems = Vec::new();
        for vendor in self.routing.reviewer_by_vendor.keys() {
            if let Err(error) = self.reviewer_profile_for_vendor(vendor) {
                problems.push(format!("routing.reviewer_by_vendor.{vendor}: {error:#}"));
            }
        }
        let layers = std::iter::once(("[review]".to_string(), &self.review)).chain(
            self.projects
                .iter()
                .map(|(k, p)| (format!("[projects.{k}.review]"), &p.review)),
        );
        for (name, layer) in layers {
            if let Some(Reviewer::Profile(p)) = layer.reviewer.as_deref().map(Reviewer::parse)
                && !profiles.contains(&p)
            {
                problems.push(format!(
                    "{name}: unknown reviewer profile '{p}' (use none, pm, cross-vendor or one of: {})",
                    profiles.join(", ")
                ));
            }
            for glob in layer.owner_paths.iter().flatten() {
                if let Err(e) = validate_glob(glob) {
                    problems.push(format!("{name}: owner_paths: {e}"));
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
