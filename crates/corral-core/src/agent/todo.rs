//! Small todo commands over the existing canonical progress shape, not a second ledger.
use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{ProgressPhase, ProgressStep, RunProgress, StepState};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TodoStep {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub criteria: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TodoPhase {
    pub id: String,
    pub label: String,
    pub steps: Vec<TodoStep>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TodoAction {
    Init { phases: Vec<TodoPhase> },
    Append { phase: String, steps: Vec<TodoStep> },
    Start { step: String },
    Done { step: String },
    Drop { step: String, reason: String },
}

fn pending(step: &TodoStep) -> ProgressStep {
    ProgressStep {
        id: step.id.clone(),
        label: step.label.clone(),
        criteria: step.criteria.clone(),
        status: StepState::Pending,
    }
}

fn transition(progress: &mut RunProgress, step: &str, status: StepState) -> Result<()> {
    let target = progress
        .phases
        .iter_mut()
        .flat_map(|p| &mut p.steps)
        .find(|s| s.id == step)
        .context("unknown_todo_step")?;
    ensure!(
        matches!(target.status, StepState::Pending | StepState::InProgress),
        "terminal_todo_step"
    );
    target.status = status;
    Ok(())
}

impl TodoAction {
    /// Apply one command, retaining terminal states and every existing phase/step identity.
    pub fn apply(&self, previous: Option<&RunProgress>, spec: &str) -> Result<RunProgress> {
        let mut progress = match self {
            Self::Init { phases } => {
                ensure!(previous.is_none(), "todo_already_initialized");
                RunProgress {
                    revision: 0,
                    spec_revision: spec.into(),
                    phases: phases
                        .iter()
                        .map(|p| ProgressPhase {
                            id: p.id.clone(),
                            label: p.label.clone(),
                            steps: p.steps.iter().map(pending).collect(),
                        })
                        .collect(),
                }
            }
            Self::Append { .. } | Self::Start { .. } | Self::Done { .. } | Self::Drop { .. } => {
                let previous = previous.context("todo_not_initialized")?;
                ensure!(previous.spec_revision == spec, "stale_todo_spec");
                previous.clone()
            }
        };
        match self {
            Self::Init { .. } => {}
            Self::Append { phase, steps } => {
                let phase = progress
                    .phases
                    .iter_mut()
                    .find(|p| p.id == *phase)
                    .context("unknown_todo_phase")?;
                phase.steps.extend(steps.iter().map(pending));
            }
            Self::Start { step } => transition(&mut progress, step, StepState::InProgress)?,
            Self::Done { step } => transition(&mut progress, step, StepState::Done)?,
            Self::Drop { step, reason } => {
                ensure!(!reason.trim().is_empty(), "drop_reason_required");
                transition(
                    &mut progress,
                    step,
                    StepState::Dropped {
                        reason: reason.clone(),
                    },
                )?;
            }
        }
        let mut phases = HashSet::new();
        let mut steps = HashSet::new();
        let mut current = 0usize;
        ensure!(!progress.phases.is_empty(), "empty_todo_phases");
        for phase in &progress.phases {
            ensure!(
                !phase.id.trim().is_empty()
                    && phases.insert(&phase.id)
                    && !phase.label.trim().is_empty(),
                "invalid_todo_phase"
            );
            for step in &phase.steps {
                ensure!(
                    !step.id.trim().is_empty()
                        && steps.insert(&step.id)
                        && !step.label.trim().is_empty(),
                    "invalid_todo_step"
                );
                if step.status == StepState::InProgress {
                    current += 1;
                }
            }
        }
        ensure!(current <= 1, "multiple_current_steps");
        progress.revision = progress
            .revision
            .checked_add(1)
            .context("progress_revision_exhausted")?;
        Ok(progress)
    }
}
