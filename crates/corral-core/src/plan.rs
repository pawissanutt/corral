//! Structured plans and their independent review history.

use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Artifact, HerdrRef, Verdict};

/// Required YAML frontmatter of a plan artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDocument {
    pub goal: String,
    pub non_goals: Vec<String>,
    pub decisions: Vec<String>,
    pub affected_paths: Vec<String>,
    pub contracts: Vec<String>,
    pub dependencies: Vec<PlanScope>,
    pub acceptance: Vec<PlanAcceptance>,
    pub verification: Vec<String>,
    pub owner_decisions: Vec<String>,
}

/// A task's write scope and prerequisites within a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanScope {
    pub task: String,
    pub write_scope: Vec<String>,
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<PlanAcceptance>,
}

/// An observable acceptance scenario, optionally linked to a criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanAcceptance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criterion: Option<String>,
    pub scenario: String,
    pub expected: String,
}

impl PlanDocument {
    /// Parses and validates YAML frontmatter, ignoring trailing Markdown.
    ///
    /// # Errors
    /// Returns an error for missing delimiters, schema mismatches, blank entries,
    /// empty required lists, or repeated criterion IDs.
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        ensure!(
            lines.next() == Some("---"),
            "missing plan frontmatter start"
        );
        let mut yaml = String::new();
        let mut closed = false;
        for line in lines {
            if line == "---" {
                closed = true;
                break;
            }
            yaml.push_str(line);
            yaml.push('\n');
        }
        ensure!(closed, "unterminated plan frontmatter");
        let document: Self = serde_yaml_ng::from_str(&yaml).context("parsing plan frontmatter")?;
        ensure!(
            !document.goal.trim().is_empty(),
            "plan goal must not be blank"
        );
        for (name, entries, required) in [
            ("non_goals", &document.non_goals, false),
            ("decisions", &document.decisions, false),
            ("affected_paths", &document.affected_paths, true),
            ("contracts", &document.contracts, false),
            ("verification", &document.verification, true),
            ("owner_decisions", &document.owner_decisions, false),
        ] {
            ensure!(!required || !entries.is_empty(), "{name} must not be empty");
            ensure!(
                entries.iter().all(|entry| !entry.trim().is_empty()),
                "{name} must not contain blank entries"
            );
        }
        for scope in &document.dependencies {
            ensure!(
                !scope.task.trim().is_empty(),
                "dependency task must not be blank"
            );
            for (name, entries) in [
                ("write_scope", &scope.write_scope),
                ("depends_on", &scope.depends_on),
            ] {
                ensure!(
                    entries.iter().all(|entry| !entry.trim().is_empty()),
                    "{name} must not contain blank entries"
                );
            }
        }
        ensure!(
            !document.acceptance.is_empty(),
            "acceptance must not be empty"
        );
        let mut criteria = BTreeSet::new();
        for acceptance in &document.acceptance {
            ensure!(
                !acceptance.scenario.trim().is_empty() && !acceptance.expected.trim().is_empty(),
                "acceptance scenario and expected must not be blank"
            );
            if let Some(criterion) = &acceptance.criterion {
                ensure!(!criterion.trim().is_empty(), "criterion must not be blank");
                ensure!(
                    criteria.insert(criterion.trim()),
                    "duplicate criterion {criterion:?}"
                );
            }
        }
        Ok(document)
    }
}

/// A snapshotted plan bound to the task specification it addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    pub id: String,
    pub artifact: Artifact,
    pub spec_revision: String,
    pub document: PlanDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<PlanReview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<PlanApproval>,
}

/// Final acceptance is distinct from the independent review verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanApproval {
    pub by: String,
    pub at: Timestamp,
    pub policy_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRunState {
    Planning,
    Submitted,
    Reviewing,
    AwaitingOwner,
    Approved,
    Changes,
    Escalated,
    Failed,
    Cancelled,
}

/// A planning session is never an implementation run or a Git candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRun {
    pub id: String,
    pub profile: String,
    pub harness: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub effort: Option<String>,
    pub spec_revision: String,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub herdr: Option<HerdrRef>,
    pub output_dir: std::path::PathBuf,
    pub submitted: Option<String>,
    pub rounds: u32,
    pub state: PlanRunState,
    pub pending_reply: Option<String>,
}

impl PlanRun {
    pub fn holds_slot(&self) -> bool {
        self.ended.is_none() && matches!(self.state, PlanRunState::Planning | PlanRunState::Changes)
    }
}

/// One independent review attempt against a plan and policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReview {
    pub id: String,
    pub reviewer: String,
    pub policy_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr: Option<HerdrRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reply: Option<String>,
}
