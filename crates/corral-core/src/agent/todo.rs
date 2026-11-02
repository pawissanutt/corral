//! Small todo commands over the existing canonical progress shape, not a second ledger.
use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{ProgressPhase, ProgressStep, RunProgress, StepState, StepWait, WaitOn};

/// Longest wait description a session ledger stores.
pub const WAIT_TEXT_MAX: usize = 256;

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

/// Every command but `init` may name the plan it updates; naming any other plan is stale.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum TodoAction {
    Init {
        phases: Vec<TodoPhase>,
    },
    Append {
        phase: String,
        /// Opens `phase` as a new last phase when it does not exist; ignored when it does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        steps: Vec<TodoStep>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Start {
        step: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Done {
        step: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    Drop {
        step: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
    /// Session ledgers only: an open step waits on a named event or an owner answer.
    Wait {
        step: String,
        on: WaitOn,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plan: Option<String>,
    },
}

fn pending(step: &TodoStep) -> ProgressStep {
    ProgressStep {
        id: step.id.clone(),
        label: step.label.clone(),
        criteria: step.criteria.clone(),
        status: StepState::Pending,
    }
}

fn open_step<'a>(progress: &'a mut RunProgress, step: &str) -> Result<&'a mut ProgressStep> {
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
    Ok(target)
}

fn transition(progress: &mut RunProgress, step: &str, status: StepState) -> Result<()> {
    open_step(progress, step)?.status = status;
    progress.waits.retain(|wait| wait.step != step);
    Ok(())
}

fn wait_text(on: &WaitOn) -> &str {
    match on {
        WaitOn::Event { event } => event,
        WaitOn::Owner { question } => question,
    }
}

impl TodoAction {
    pub fn plan(&self) -> Option<&str> {
        match self {
            Self::Init { .. } => None,
            Self::Append { plan, .. }
            | Self::Start { plan, .. }
            | Self::Done { plan, .. }
            | Self::Drop { plan, .. }
            | Self::Wait { plan, .. } => plan.as_deref(),
        }
    }

    /// A command that names a plan must name the ledger's current one.
    pub fn check_plan(&self, current: Option<&str>) -> Result<()> {
        if let Some(named) = self.plan() {
            ensure!(Some(named) == current, "stale_todo_plan");
        }
        Ok(())
    }

    /// Apply one command to a worker run's ledger, retaining terminal states and every existing
    /// phase/step identity. Run progress has no plan identity and no waits.
    pub fn apply(&self, previous: Option<&RunProgress>, spec: &str) -> Result<RunProgress> {
        if matches!(self, Self::Wait { .. }) {
            bail!("todo_wait_needs_a_session_ledger");
        }
        self.check_plan(None)?;
        let base = match self {
            Self::Init { .. } => {
                ensure!(previous.is_none(), "todo_already_initialized");
                None
            }
            _ => {
                let previous = previous.context("todo_not_initialized")?;
                ensure!(previous.spec_revision == spec, "stale_todo_spec");
                Some(previous)
            }
        };
        self.step(base, spec, 0)
    }

    /// Apply the command to `base`; an init replaces it and continues after revision `floor`,
    /// so a ledger's revision never moves backwards across successive plans.
    pub(crate) fn step(
        &self,
        base: Option<&RunProgress>,
        spec: &str,
        floor: u64,
    ) -> Result<RunProgress> {
        let mut progress = match (self, base) {
            (Self::Init { phases }, _) => RunProgress {
                revision: floor,
                spec_revision: spec.into(),
                phases: phases
                    .iter()
                    .map(|p| ProgressPhase {
                        id: p.id.clone(),
                        label: p.label.clone(),
                        steps: p.steps.iter().map(pending).collect(),
                    })
                    .collect(),
                plan: None,
                waits: Vec::new(),
            },
            (_, Some(base)) => base.clone(),
            (_, None) => bail!("todo_not_initialized"),
        };
        match self {
            Self::Init { .. } => {}
            Self::Append {
                phase,
                label,
                steps,
                ..
            } => {
                let steps = steps.iter().map(pending);
                match progress.phases.iter_mut().find(|p| p.id == *phase) {
                    Some(existing) => existing.steps.extend(steps),
                    None => progress.phases.push(ProgressPhase {
                        id: phase.clone(),
                        label: label.clone().context("unknown_todo_phase")?,
                        steps: steps.collect(),
                    }),
                }
            }
            Self::Start { step, .. } => transition(&mut progress, step, StepState::InProgress)?,
            Self::Done { step, .. } => transition(&mut progress, step, StepState::Done)?,
            Self::Drop { step, reason, .. } => {
                ensure!(!reason.trim().is_empty(), "drop_reason_required");
                transition(
                    &mut progress,
                    step,
                    StepState::Dropped {
                        reason: reason.clone(),
                    },
                )?;
            }
            Self::Wait { step, on, .. } => {
                let text = wait_text(on);
                ensure!(
                    !text.trim().is_empty()
                        && text.len() <= WAIT_TEXT_MAX
                        && !text.chars().any(char::is_control),
                    "invalid_todo_wait"
                );
                open_step(&mut progress, step)?;
                progress.waits.retain(|wait| wait.step != *step);
                progress.waits.push(StepWait {
                    step: step.clone(),
                    on: on.clone(),
                });
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
