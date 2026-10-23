//! Container naming, the pure liveness and fence predicate, and the closure predicate (F2, F4).
use serde::{Deserialize, Serialize};

use super::{AgentPm, AgentRunState};
use crate::{Run, RunOutcome, Status, Task};

/// The named container of a request. Lookup resolves it even when it is historical.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContainerRef {
    Worker {
        task: String,
        run: String,
    },
    Reviewer {
        task: String,
        run: String,
        review: String,
    },
    Pm,
}

impl ContainerRef {
    /// `<task>/<run>`, `<task>/<run>/<review>` or `pm`; the prefix of every question key.
    pub fn key(&self) -> String {
        match self {
            ContainerRef::Worker { task, run } => format!("{task}/{run}"),
            ContainerRef::Reviewer { task, run, review } => format!("{task}/{run}/{review}"),
            ContainerRef::Pm => "pm".into(),
        }
    }

    pub fn task(&self) -> Option<&str> {
        match self {
            ContainerRef::Worker { task, .. } | ContainerRef::Reviewer { task, .. } => Some(task),
            ContainerRef::Pm => None,
        }
    }
}

/// The requester's binding, checked by `check_live` in every final transaction. It lives
/// only in memory for the call being executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBinding {
    pub container: ContainerRef,
    pub generation: u32,
    /// Reviewer only: the callback token the envelope named.
    pub callback: Option<String>,
    /// The turn an `event turn_ended` settles. Only that turn, started and still open in this
    /// generation, stays admissible after done moved the task to Review or a verdict closed
    /// the review child.
    pub settles_turn: Option<u64>,
}

impl RequestBinding {
    fn settles_open_turn(&self, state: Option<&AgentRunState>) -> bool {
        self.settles_turn.is_some_and(|seq| {
            state.is_some_and(|state| {
                state.turns.iter().any(|t| {
                    t.generation == self.generation
                        && t.seq == seq
                        && t.started.is_some()
                        && t.ended.is_none()
                })
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    AgentFenced,
    RunEnded,
    ReviewClosed,
}

impl RefusalCode {
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalCode::AgentFenced => "agent_fenced",
            RefusalCode::RunEnded => "run_ended",
            RefusalCode::ReviewClosed => "review_closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl AgentRefusal {
    fn new(code: RefusalCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for AgentRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "agent_refusal={}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for AgentRefusal {}

/// Why a container no longer holds pending questions (F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureReason {
    RunEnded,
    ReviewClosed,
    ReviewSuperseded,
    ReviewInterrupted,
}

impl ClosureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ClosureReason::RunEnded => "run_ended",
            ClosureReason::ReviewClosed => "review_closed",
            ClosureReason::ReviewSuperseded => "review_superseded",
            ClosureReason::ReviewInterrupted => "review_interrupted",
        }
    }
}

/// The worker run or review child a container names, current or historical.
pub fn container_run<'a>(task: &'a Task, container: &ContainerRef) -> Option<&'a Run> {
    match container {
        ContainerRef::Worker { task: id, run } => {
            (task.id == *id).then(|| task.runs.iter().find(|r| r.id == *run))?
        }
        ContainerRef::Reviewer {
            task: id,
            run,
            review,
        } => (task.id == *id)
            .then(|| task.runs.iter().find(|r| r.id == *run))??
            .review_runs
            .iter()
            .find(|c| c.id == *review),
        ContainerRef::Pm => None,
    }
}

pub fn container_run_mut<'a>(task: &'a mut Task, container: &ContainerRef) -> Option<&'a mut Run> {
    match container {
        ContainerRef::Worker { task: id, run } => {
            if task.id != *id {
                return None;
            }
            task.runs.iter_mut().find(|r| r.id == *run)
        }
        ContainerRef::Reviewer {
            task: id,
            run,
            review,
        } => {
            if task.id != *id {
                return None;
            }
            task.runs
                .iter_mut()
                .find(|r| r.id == *run)?
                .review_runs
                .iter_mut()
                .find(|c| c.id == *review)
        }
        ContainerRef::Pm => None,
    }
}

pub fn container_state<'a>(task: &'a Task, container: &ContainerRef) -> Option<&'a AgentRunState> {
    container_run(task, container)?.agent_host.as_deref()
}

pub fn container_state_mut<'a>(
    task: &'a mut Task,
    container: &ContainerRef,
) -> Option<&'a mut AgentRunState> {
    container_run_mut(task, container)?
        .agent_host
        .as_deref_mut()
}

/// A worker run ends only by discard, retry, drop or landing; done in Review, a refused
/// operation, a cancelled turn and host loss leave it open.
fn worker_closed(task: &Task, run: &str) -> bool {
    let Some(named) = task.runs.iter().find(|r| r.id == run) else {
        return true;
    };
    task.current_run().is_none_or(|current| current.id != run)
        || task.status == Status::Dropped
        || matches!(
            named.outcome,
            Some(RunOutcome::Discarded | RunOutcome::Superseded | RunOutcome::Abandoned)
        )
        || (named.outcome == Some(RunOutcome::Done) && task.status == Status::Done)
}

/// F4 closure predicate, pure over one task record. `None` while the container is open.
pub fn container_closure(task: &Task, container: &ContainerRef) -> Option<ClosureReason> {
    match container {
        ContainerRef::Worker { task: id, run } => {
            (task.id != *id || worker_closed(task, run)).then_some(ClosureReason::RunEnded)
        }
        ContainerRef::Reviewer {
            task: id,
            run,
            review,
        } => {
            if task.id != *id || worker_closed(task, run) {
                return Some(ClosureReason::RunEnded);
            }
            let parent = task.runs.iter().find(|r| r.id == *run)?;
            let Some(index) = parent.review_runs.iter().position(|c| c.id == *review) else {
                return Some(ClosureReason::ReviewSuperseded);
            };
            let child = &parent.review_runs[index];
            let last = index + 1 == parent.review_runs.len();
            let active = parent.review.as_ref().filter(|_| last);
            let records = || child.review.iter().chain(active);
            if records().any(|r| r.verdict.is_some()) {
                Some(ClosureReason::ReviewClosed)
            } else if child.outcome == Some(RunOutcome::Superseded) {
                Some(ClosureReason::ReviewSuperseded)
            } else if child.outcome.is_some() || records().any(|r| r.process_lost) {
                Some(ClosureReason::ReviewInterrupted)
            } else if !last {
                Some(ClosureReason::ReviewSuperseded)
            } else {
                None
            }
        }
        ContainerRef::Pm => None,
    }
}

fn generation_live(state: &AgentRunState, generation: u32) -> Result<(), AgentRefusal> {
    if state.binding.generation != generation {
        return Err(AgentRefusal::new(
            RefusalCode::AgentFenced,
            format!(
                "generation {generation} is not current (current {})",
                state.binding.generation
            ),
        ));
    }
    match state.generation(generation) {
        None => Err(AgentRefusal::new(
            RefusalCode::AgentFenced,
            format!("generation {generation} was never opened"),
        )),
        Some(g) if g.fenced.is_some() => Err(AgentRefusal::new(
            RefusalCode::AgentFenced,
            format!("generation {generation} is fenced"),
        )),
        Some(_) => Ok(()),
    }
}

/// The liveness and fence predicate of every new mutation, pure over one task record: the
/// named run is current and accepts work, a named review child is the open last attempt, and
/// the generation is current and unfenced. Final transactions and the gate submission
/// boundary call it under the task lock.
pub fn check_live(task: &Task, binding: &RequestBinding) -> Result<(), AgentRefusal> {
    let ended = |detail: &str| AgentRefusal::new(RefusalCode::RunEnded, detail);
    let (run_id, review) = match &binding.container {
        ContainerRef::Worker { task: id, run } if *id == task.id => (run, None),
        ContainerRef::Reviewer {
            task: id,
            run,
            review,
        } if *id == task.id => (run, Some(review)),
        _ => return Err(ended("the binding does not name this task")),
    };
    if worker_closed(task, run_id) {
        return Err(ended("the named run is no longer current or has ended"));
    }
    let parent = task
        .runs
        .iter()
        .find(|r| r.id == *run_id)
        .ok_or_else(|| ended("the named run does not exist"))?;
    let state = match review {
        None => {
            let open = parent.outcome.is_none() && task.status.holds_slot();
            let settling = binding.settles_open_turn(parent.agent_host.as_deref())
                && parent.outcome == Some(RunOutcome::Done)
                && task.status == Status::Review;
            if !open && !settling {
                return Err(ended("the named run no longer accepts work"));
            }
            parent.agent_host.as_deref()
        }
        Some(_) => {
            let closed = |detail: String| AgentRefusal::new(RefusalCode::ReviewClosed, detail);
            let child_state = container_state(task, &binding.container);
            let settling = match container_closure(task, &binding.container) {
                None => false,
                Some(ClosureReason::RunEnded) => return Err(ended("the parent run has ended")),
                Some(ClosureReason::ReviewClosed) if binding.settles_open_turn(child_state) => true,
                Some(reason) => return Err(closed(format!("review child {}", reason.as_str()))),
            };
            if !settling && task.status != Status::Review {
                return Err(closed("the task is no longer in Review".into()));
            }
            let child = container_run(task, &binding.container)
                .ok_or_else(|| closed("the named review child does not exist".into()))?;
            let state = child.agent_host.as_deref();
            if state.and_then(|s| s.binding.callback.as_ref()) != binding.callback.as_ref() {
                return Err(closed(
                    "the callback token does not match the bound attempt".into(),
                ));
            }
            state
        }
    };
    let state = state.ok_or_else(|| {
        AgentRefusal::new(
            RefusalCode::AgentFenced,
            "the container has no agent binding",
        )
    })?;
    generation_live(state, binding.generation)
}

/// The PM's liveness and fence predicate over the AgentPm record.
pub fn check_pm_live(pm: &AgentPm, generation: u32) -> Result<(), AgentRefusal> {
    generation_live(&pm.state, generation)
}
