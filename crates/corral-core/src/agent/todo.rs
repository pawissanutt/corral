//! Small todo commands over the existing canonical progress shape, not a second ledger.
use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{ProgressPhase, ProgressStep, RunProgress, StepState, StepWait, WaitOn};

/// Longest wait description a session ledger stores.
pub const WAIT_TEXT_MAX: usize = 256;
/// Default review intervals for event and owner waits in a session ledger.
pub const EVENT_WAIT_DEADLINE: SignedDuration = SignedDuration::from_hours(2);
pub const OWNER_WAIT_DEADLINE: SignedDuration = SignedDuration::from_hours(24);
const WAIT_DEADLINE_MAX: SignedDuration = SignedDuration::from_hours(7 * 24);

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
        deadline: Option<Timestamp>,
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
        self.step(base, spec, 0, None)
    }

    /// Apply the command to `base`; an init replaces it and continues after revision `floor`,
    /// so a ledger's revision never moves backwards across successive plans.
    /// Session callers supply their timestamp; worker ledgers cannot wait and supply none.
    pub(crate) fn step(
        &self,
        base: Option<&RunProgress>,
        spec: &str,
        floor: u64,
        now: Option<Timestamp>,
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
            Self::Wait {
                step, on, deadline, ..
            } => {
                let text = wait_text(on);
                ensure!(
                    !text.trim().is_empty()
                        && text.len() <= WAIT_TEXT_MAX
                        && !text.chars().any(char::is_control),
                    "invalid_todo_wait"
                );
                let now = now.context("todo_wait_needs_a_session_ledger")?;
                let deadline = match deadline {
                    Some(deadline) => *deadline,
                    None => now
                        .checked_add(match on {
                            WaitOn::Event { .. } => EVENT_WAIT_DEADLINE,
                            WaitOn::Owner { .. } => OWNER_WAIT_DEADLINE,
                        })
                        .context("todo_wait_deadline_range")?,
                };
                ensure!(
                    deadline >= now && deadline.duration_since(now) <= WAIT_DEADLINE_MAX,
                    "todo_wait_deadline_range"
                );
                open_step(&mut progress, step)?;
                progress.waits.retain(|wait| wait.step != *step);
                progress.waits.push(StepWait {
                    step: step.clone(),
                    on: on.clone(),
                    deadline: Some(deadline),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::request::RequestLedger;

    #[test]
    fn session_waits_store_bounded_deadlines_and_load_legacy_waits() {
        let now: Timestamp = "2026-11-11T12:00:00Z".parse().unwrap();
        let mut requests = RequestLedger::default();
        let init: TodoAction = serde_json::from_value(json!({
            "op": "init",
            "phases": [{"id": "work", "label": "Work",
                "steps": [{"id": "check", "label": "Check"}]}]
        }))
        .unwrap();
        let base = requests.apply(&init, None, "pm:corral", now).unwrap();
        for (on, duration) in [
            (
                json!({"kind": "event", "event": "gate finishes"}),
                EVENT_WAIT_DEADLINE,
            ),
            (
                json!({"kind": "owner", "question": "Which scope?"}),
                OWNER_WAIT_DEADLINE,
            ),
        ] {
            let mut payload = json!({"op": "wait", "step": "check", "on": on});
            let action: TodoAction = serde_json::from_value(payload.clone()).unwrap();
            let progress = requests
                .apply(&action, Some(&base), "pm:corral", now)
                .unwrap();
            assert_eq!(
                progress.waits[0].deadline,
                Some(now.checked_add(duration).unwrap())
            );

            // A rollback's strict parser must accept the entire stored wait unchanged.
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct OldStepWait {
                step: String,
                on: WaitOn,
            }
            let stored = serde_json::to_value(&progress).unwrap();
            let old: OldStepWait = serde_json::from_value(stored["waits"][0].clone()).unwrap();
            assert_eq!(old.step, progress.waits[0].step);
            assert_eq!(old.on, progress.waits[0].on);
            let legacy: RunProgress = serde_json::from_value(stored).unwrap();
            assert_eq!(legacy.waits[0].deadline, None);

            for offset in [
                SignedDuration::from_nanos(-1),
                WAIT_DEADLINE_MAX + SignedDuration::from_nanos(1),
            ] {
                payload["deadline"] = json!(now.checked_add(offset).unwrap());
                let action: TodoAction = serde_json::from_value(payload.clone()).unwrap();
                assert_eq!(
                    requests
                        .apply(&action, Some(&base), "pm:corral", now)
                        .unwrap_err()
                        .to_string(),
                    "todo_wait_deadline_range"
                );
            }
            for offset in [SignedDuration::ZERO, WAIT_DEADLINE_MAX] {
                let deadline = now.checked_add(offset).unwrap();
                payload["deadline"] = json!(deadline);
                let action: TodoAction = serde_json::from_value(payload.clone()).unwrap();
                let explicit = requests
                    .apply(&action, Some(&base), "pm:corral", now)
                    .unwrap();
                assert_eq!(explicit.waits[0].deadline, Some(deadline));
            }
        }
    }
}
