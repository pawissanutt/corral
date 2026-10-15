//! Review policy: the `[review]` table, overridable per project (`[projects.<p>.review]`) and per task
//! (frontmatter `review: owner|auto`). The default policy is the manual flow: no gate, no reviewer, the owner
//! accepts.

use anyhow::{Result, bail};
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Reviewer {
    #[default]
    None,
    /// The workspace PM agent.
    Pm,
    /// An independent agent launched with this profile.
    Profile(String),
}

impl Reviewer {
    /// "none" and "pm" are keywords; anything else names a profile.
    pub fn parse(s: &str) -> Reviewer {
        match s.trim() {
            "none" => Reviewer::None,
            "pm" => Reviewer::Pm,
            other => Reviewer::Profile(other.to_string()),
        }
    }
}

/// One layer of review settings (global `[review]` or `[projects.<p>.review]`); unset keys inherit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOverride {
    /// Shell commands run in the run worktree, in order; empty = no gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Vec<String>>,
    /// "none" | "pm" | "<profile>".
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
}

impl ReviewOverride {
    pub fn is_empty(&self) -> bool {
        *self == ReviewOverride::default()
    }

    fn apply(&self, p: &mut ReviewPolicy) {
        if let Some(v) = &self.gate {
            p.gate = v.clone();
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPolicy {
    pub gate: Vec<String>,
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
            gate: Vec::new(),
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
                    "{name}: unknown reviewer profile '{p}' (use none, pm or one of: {})",
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
        }
        problems
    }
}
