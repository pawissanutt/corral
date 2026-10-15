//! Harness-neutral execution progress. Step completion is not acceptance evidence.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Criterion, EvidenceRecord, Run, RunObligations, Task};

/// Read-only join of canonical criteria, execution steps, and acceptance receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriterionProgress {
    pub criterion: Criterion,
    pub steps: Vec<ProgressStep>,
    pub evidence: Vec<EvidenceRecord>,
}

pub fn criterion_progress(
    task: &Task,
    run: &Run,
    progress: Option<&RunProgress>,
) -> Vec<CriterionProgress> {
    task.completion
        .iter()
        .flat_map(|m| &m.criteria)
        .map(|criterion| CriterionProgress {
            criterion: criterion.clone(),
            steps: progress
                .into_iter()
                .flat_map(|p| p.steps())
                .filter(|s| s.criteria.contains(&criterion.id))
                .cloned()
                .collect(),
            evidence: run
                .completion
                .evidence
                .iter()
                .filter(|e| e.observation.criterion == criterion.id)
                .cloned()
                .collect(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepState {
    Pending,
    InProgress,
    Done,
    Dropped { reason: String },
}

impl StepState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Dropped { .. } => "dropped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressStep {
    pub id: String,
    pub label: String,
    pub status: StepState,
    #[serde(default)]
    pub criteria: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressPhase {
    pub id: String,
    pub label: String,
    pub steps: Vec<ProgressStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressUpdate {
    pub task: String,
    pub run: String,
    pub spec_revision: String,
    /// Compare-and-swap revision. Zero creates the first progress record.
    pub expected_revision: u64,
    pub phases: Vec<ProgressPhase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgress {
    pub revision: u64,
    pub spec_revision: String,
    pub phases: Vec<ProgressPhase>,
}

impl RunProgress {
    /// A criterion is execution-complete only when every linked step is done.
    pub fn criterion_complete(&self, id: &str) -> bool {
        let mut linked = self.steps().filter(|s| s.criteria.iter().any(|c| c == id));
        linked.next().is_some_and(|s| s.status == StepState::Done)
            && linked.all(|s| s.status == StepState::Done)
    }

    pub fn steps(&self) -> impl Iterator<Item = &ProgressStep> {
        self.phases.iter().flat_map(|p| &p.steps)
    }

    pub fn current_step(&self) -> Option<&ProgressStep> {
        self.steps().find(|s| s.status == StepState::InProgress)
    }

    /// The current phase, or the first phase with open work, then the last phase.
    pub fn summary(&self) -> String {
        let phase = self
            .phases
            .iter()
            .find(|p| p.steps.iter().any(|s| s.status == StepState::InProgress))
            .or_else(|| {
                self.phases
                    .iter()
                    .find(|p| p.steps.iter().any(|s| s.status == StepState::Pending))
            })
            .or_else(|| self.phases.last());
        let done = self.steps().filter(|s| s.status == StepState::Done).count();
        format!(
            "{} {done}/{}",
            phase.map_or("Progress", |p| p.label.as_str()),
            self.steps().count()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitObligation {
    pub id: String,
    pub owner: String,
    pub handle: String,
    pub condition: String,
    /// Keep the obligation in history; resolving it requires a concrete receipt/reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceObligation {
    pub id: String,
    pub owner: String,
    pub description: String,
    pub cleanup: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointUpdate {
    pub task: String,
    pub run: String,
    pub spec_revision: String,
    pub expected_revision: u64,
    pub progress_revision: u64,
    pub head: String,
    /// Current integration base commit, not the original dispatch base.
    pub base: String,
    /// Digest of HEAD plus tracked and untracked worktree content.
    pub tree: String,
    pub criterion_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub next_action: String,
    pub decisions: Vec<String>,
    pub waits: Vec<WaitObligation>,
    pub resources: Vec<ResourceObligation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub version: u32,
    pub revision: u64,
    pub input: CheckpointUpdate,
    pub progress: Option<RunProgress>,
    pub completed_steps: Vec<String>,
    pub current_step: Option<String>,
    /// Frozen native waits and decisions; their receipts remain owned by the source run.
    #[serde(default, skip_serializing_if = "RunObligations::is_empty")]
    pub run_obligations: RunObligations,
    pub captured: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub checkpoint: Checkpoint,
    pub artifact: String,
}

/// Readback includes server-computed identity for the next guarded write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeStatus {
    Missing,
    Compatible,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressReadback {
    pub task: String,
    pub run: String,
    pub spec_revision: String,
    pub head: String,
    pub base: String,
    pub tree: String,
    pub progress: Option<RunProgress>,
    pub criteria: Vec<CriterionProgress>,
    pub evidence_failures: Vec<String>,
    pub checkpoints: Vec<CheckpointRecord>,
    pub checkpoint_status: ResumeStatus,
    pub checkpoint_reasons: Vec<String>,
}
