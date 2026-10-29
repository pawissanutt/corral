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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_target: Option<PlanDeliveryTarget>,
    /// Presence opts into the human-review format's structural requirements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_review: Option<PlanOverview>,
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

/// Human-facing context; goal, exclusions, choices and acceptance stay canonical
/// in the corresponding [`PlanDocument`] fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOverview {
    /// The problem this plan addresses.
    pub problem: String,
    /// Included changes, stated as outcomes rather than implementation steps.
    pub changes: Vec<String>,
    /// Existing behavior retained; an explicit "None" is valid.
    pub retained: Vec<String>,
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
    /// Member gate; only in integration-target plans with two or more scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<crate::ScopeGate>,
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

/// A Plan's intended delivery destination, not execution or landing authority.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlanDeliveryTarget {
    #[default]
    Main,
    Integration {
        branch: String,
    },
}

/// Immutable execution target selected when a Plan member is dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPlanTarget {
    pub plan: String,
    pub scope_revision: u64,
    pub contract_revision: String,
    pub branch: String,
    pub checkout: std::path::PathBuf,
}

/// Approved aggregate scope carried by a tracker, separate from design history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedScope {
    pub goal: String,
    pub acceptance: Vec<PlanAcceptance>,
    #[serde(default)]
    pub delivery_target: PlanDeliveryTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_design: Option<String>,
}

/// The exact previous scope retained when an explicit amendment replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedScopeRevision {
    pub revision: u64,
    pub approved_scope: Option<ApprovedScope>,
    pub superseded_at: Timestamp,
}

pub(crate) fn is_scope_revision_zero(revision: &u64) -> bool {
    *revision == 0
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
        if let Some(overview) = &document.human_review {
            ensure!(
                !overview.problem.trim().is_empty(),
                "human_review.problem must not be blank"
            );
            for (name, entries) in [
                ("human_review.changes", &overview.changes),
                ("human_review.retained", &overview.retained),
            ] {
                ensure!(!entries.is_empty(), "{name} must not be empty");
                ensure!(
                    entries.iter().all(|entry| !entry.trim().is_empty()),
                    "{name} must not contain blank entries"
                );
            }
        }
        for (name, entries, required) in [
            (
                "non_goals",
                &document.non_goals,
                document.human_review.is_some(),
            ),
            (
                "decisions",
                &document.decisions,
                document.human_review.is_some(),
            ),
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
            if let Some(gate) = &scope.gate {
                ensure!(
                    matches!(
                        document.delivery_target,
                        Some(PlanDeliveryTarget::Integration { .. })
                    ) && document.dependencies.len() >= 2,
                    "scope {:?}: gate is valid only in an integration-target plan with two or more scopes",
                    scope.task
                );
                gate.validate()
                    .with_context(|| format!("scope {:?} gate", scope.task))?;
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
    /// Exact submitted acceptance action; legacy settings alone never establish this proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<Box<crate::agent::AgentRunState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<crate::ExecutionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::RouteSource>,
    /// Fixed at launch; runs predating inbox adapters retain legacy delivery.
    #[serde(default)]
    pub delivery: crate::inbox::DeliveryMode,
    #[serde(default)]
    pub inbox: crate::inbox::InboxState,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<Box<crate::agent::AgentRunState>>,
    /// Fixed at launch; reviews predating inbox adapters retain legacy delivery.
    #[serde(default)]
    pub delivery: crate::inbox::DeliveryMode,
    #[serde(default)]
    pub inbox: crate::inbox::InboxState,
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
    /// Contract, capability and reviewer selection for this attempt; absent on legacy records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Box<crate::ReviewRoute>>,
}
