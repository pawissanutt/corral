//! Assistant-local design records. These are not board plans, approvals or task runs.
//! Immutable bytes/history live in service-owned evidence records; the container holds only
//! the current bounded index. Lifecycle services must CAS both revision and authority epoch.
use anyhow::{Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::assistant::OwnerInput;
use crate::blob_ref::{BlobHash, BlobRef};

pub const WORKFLOW_VERSION: u32 = 1;
pub const CONTAINER_VERSION: u32 = 2;
pub const DOCUMENT_MAX: usize = 1024 * 1024;
pub const CHUNK_MAX: usize = 16 * 1024;
pub const HISTORY_MAX: u32 = 4096;
pub const DECISIONS_MAX: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignArtifact {
    pub blob: BlobRef,
    pub bytes: u32,
}

impl DesignArtifact {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.bytes > 0 && self.bytes as usize <= DOCUMENT_MAX,
            "invalid workflow artifact length"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Ordinary,
    Entering,
    Recovering,
    Planning,
    Executing,
    Candidate,
    Paused,
    HandoffPreparing,
    HandoffReconciling,
    BoardOwned,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeOwner {
    Local,
    Board { task: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignStage {
    Draft,
    Ready,
}

/// Relative paths only. This is a declared scope, never a filesystem permission grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionScope {
    pub project: String,
    pub paths: Vec<String>,
    pub actions: Vec<String>,
}

impl ExecutionScope {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.project.is_empty() && self.project.len() <= 128,
            "invalid scope project"
        );
        ensure!(
            !self.paths.is_empty() && self.paths.len() <= 64,
            "scope requires 1-64 paths"
        );
        for path in &self.paths {
            ensure!(
                !path.is_empty()
                    && path.len() <= 1024
                    && !path.contains(['\\', ':'])
                    && !path.chars().any(char::is_control)
                    && path.split('/').all(|part| !matches!(part, "" | "." | "..")
                        && ![".git", ".corral", ".worktrees"]
                            .iter()
                            .any(|name| part.eq_ignore_ascii_case(name))),
                "unsafe execution scope path"
            );
        }
        ensure!(
            !self.actions.is_empty()
                && self.actions.len() <= 32
                && self
                    .actions
                    .iter()
                    .all(|action| !action.trim().is_empty() && action.len() <= 1024),
            "scope requires bounded concrete actions"
        );
        Ok(())
    }
}

/// Checkable observations (including absence). Execution revalidates these, never trusts a
/// model's assertion that a branch name or a digest still matches the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAssumptions {
    pub project: String,
    pub source_head: String,
    pub base_head: String,
    pub files: Vec<SourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFile {
    pub path: String,
    /// None means the path was absent, not that its content is irrelevant.
    pub sha256: Option<BlobHash>,
}

impl SourceAssumptions {
    pub fn validate(&self, scope: &ExecutionScope) -> Result<()> {
        ensure!(
            self.project == scope.project,
            "assumptions project differs from scope"
        );
        for head in [&self.source_head, &self.base_head] {
            ensure!(
                matches!(head.len(), 40 | 64)
                    && head
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "invalid source commit"
            );
        }
        ensure!(
            !self.files.is_empty() && self.files.len() <= 128,
            "ready design requires bounded source observations"
        );
        for file in &self.files {
            ExecutionScope {
                project: self.project.clone(),
                paths: vec![file.path.clone()],
                actions: vec!["observe".into()],
            }
            .validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRevision {
    pub revision: u32,
    pub artifact: DesignArtifact,
    pub stage: DesignStage,
    pub assumptions: Option<DesignArtifact>,
    pub execution_scope: Option<DesignArtifact>,
    pub published_at: Timestamp,
    pub supersedes: Option<u32>,
    pub scope_changes: String,
}

/// Exact context used by the host/T-743 adapter. Text or an owner-input ID alone is not an
/// executable action. Consumers must authenticate the actual selected stable option once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionBinding {
    pub assistant: String,
    pub workflow: String,
    pub question: String,
    pub question_revision: u32,
    pub purpose: String,
    pub plan_revision: Option<u32>,
    pub plan_digest: Option<BlobHash>,
    pub authority_epoch: u64,
    pub action_payload_digest: BlobHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionEvidence {
    pub binding: DecisionBinding,
    pub selected_option: String,
    pub owner_input: OwnerInput,
    pub evidence_digest: BlobHash,
    pub consumed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub id: String,
    pub state_revision: u64,
    pub authority_epoch: u64,
    pub phase: WorkflowPhase,
    pub owner: ScopeOwner,
    pub original_request: DesignArtifact,
    /// Copied provenance survives the ordinary owner-input journal's eviction.
    pub owner_evidence: OwnerInput,
    pub current_plan: Option<PlanRevision>,
    pub decision_refs: Vec<DesignArtifact>,
    pub execution: Option<DesignArtifact>,
    pub reviews: Vec<DesignArtifact>,
    pub handoff: Option<DesignArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowIndex {
    pub version: u32,
    pub issued: u64,
    /// Session-scoped and monotone across closed/replaced workflows.
    pub authority_epoch: u64,
    pub current: Workflow,
    pub history_refs: Vec<DesignArtifact>,
}

impl WorkflowIndex {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == WORKFLOW_VERSION,
            "unsupported assistant workflow version"
        );
        let w = &self.current;
        ensure!(
            self.issued > 0
                && w.id == format!("W{}", self.issued)
                && self.authority_epoch > 0
                && w.authority_epoch == self.authority_epoch
                && w.state_revision > 0,
            "invalid assistant workflow identity/epoch"
        );
        ensure!(
            w.decision_refs.len() <= DECISIONS_MAX
                && w.reviews.len() <= HISTORY_MAX as usize
                && self.history_refs.len() <= HISTORY_MAX as usize,
            "assistant workflow history capacity exceeded"
        );
        w.original_request.validate()?;
        ensure!(
            !w.owner_evidence.truncated
                && w.owner_evidence.text.len() <= crate::assistant::OWNER_INPUT_TEXT_MAX
                && w.owner_evidence.sha256 == w.original_request.blob.hash().as_str()
                && BlobHash::of(w.owner_evidence.text.as_bytes())
                    == *w.original_request.blob.hash(),
            "invalid original-request provenance"
        );
        for artifact in self
            .history_refs
            .iter()
            .chain(&w.decision_refs)
            .chain(&w.reviews)
            .chain(w.execution.iter())
            .chain(w.handoff.iter())
        {
            artifact.validate()?;
        }
        if let Some(plan) = &w.current_plan {
            ensure!(
                plan.revision > 0 && plan.revision <= HISTORY_MAX,
                "invalid design revision"
            );
            ensure!(
                plan.supersedes == plan.revision.checked_sub(1).filter(|r| *r > 0)
                    && plan.scope_changes.len() <= 2048,
                "invalid design history"
            );
            plan.artifact.validate()?;
            for artifact in plan.assumptions.iter().chain(plan.execution_scope.iter()) {
                artifact.validate()?;
            }
            ensure!(
                plan.stage != DesignStage::Ready
                    || (plan.assumptions.is_some() && plan.execution_scope.is_some()),
                "ready design lacks scope/assumptions"
            );
        }
        if let ScopeOwner::Board { task } = &w.owner {
            ensure!(
                task.starts_with("T-")
                    && crate::Store::normalize_id(task).ok().as_deref() == Some(task.as_str()),
                "invalid board owner"
            );
        }
        ensure!(
            !matches!(w.phase, WorkflowPhase::BoardOwned)
                || matches!(w.owner, ScopeOwner::Board { .. }),
            "board ownership missing"
        );
        Ok(())
    }
}
