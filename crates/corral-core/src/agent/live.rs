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
    Planner {
        task: String,
        run: String,
    },
    PlanReviewer {
        task: String,
        plan: String,
        review: String,
    },
    Pm,
}

impl ContainerRef {
    /// Container key; planning IDs are namespaced with `plan-`.
    pub fn key(&self) -> String {
        match self {
            ContainerRef::Worker { task, run } => format!("{task}/{run}"),
            ContainerRef::Reviewer { task, run, review } => format!("{task}/{run}/{review}"),
            ContainerRef::Planner { task, run } => format!("{task}/plan-{run}"),
            ContainerRef::PlanReviewer { task, plan, review } => {
                format!("{task}/plan-{plan}/{review}")
            }
            ContainerRef::Pm => "pm".into(),
        }
    }

    pub fn task(&self) -> Option<&str> {
        match self {
            ContainerRef::Worker { task, .. }
            | ContainerRef::Reviewer { task, .. }
            | ContainerRef::Planner { task, .. }
            | ContainerRef::PlanReviewer { task, .. } => Some(task),
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

/// The implementation run or review child, current or historical; never a planning record.
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
        ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } | ContainerRef::Pm => None,
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
        ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } | ContainerRef::Pm => None,
    }
}

pub fn container_state<'a>(task: &'a Task, container: &ContainerRef) -> Option<&'a AgentRunState> {
    if container.task()? != task.id {
        return None;
    }
    match container {
        ContainerRef::Planner { run, .. } => task
            .plan_runs
            .iter()
            .find(|r| r.id == *run)?
            .agent_host
            .as_deref(),
        ContainerRef::PlanReviewer { plan, review, .. } => task
            .plans
            .iter()
            .find(|p| p.id == *plan)?
            .reviews
            .iter()
            .find(|r| r.id == *review)?
            .agent_host
            .as_deref(),
        ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. } => {
            container_run(task, container)?.agent_host.as_deref()
        }
        ContainerRef::Pm => None,
    }
}

pub fn container_state_mut<'a>(
    task: &'a mut Task,
    container: &ContainerRef,
) -> Option<&'a mut AgentRunState> {
    container_host_mut(task, container)?.as_deref_mut()
}

/// The host slot of a named container, including an unbound or historical one.
pub fn container_host_mut<'a>(
    task: &'a mut Task,
    container: &ContainerRef,
) -> Option<&'a mut Option<Box<AgentRunState>>> {
    if container.task()? != task.id {
        return None;
    }
    match container {
        ContainerRef::Planner { run, .. } => {
            Some(&mut task.plan_runs.iter_mut().find(|r| r.id == *run)?.agent_host)
        }
        ContainerRef::PlanReviewer { plan, review, .. } => Some(
            &mut task
                .plans
                .iter_mut()
                .find(|p| p.id == *plan)?
                .reviews
                .iter_mut()
                .find(|r| r.id == *review)?
                .agent_host,
        ),
        ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. } => {
            Some(&mut container_run_mut(task, container)?.agent_host)
        }
        ContainerRef::Pm => None,
    }
}

/// The recorded pane, independent of whether the host has completed binding.
pub fn container_pane<'a>(task: &'a Task, container: &ContainerRef) -> Option<&'a crate::HerdrRef> {
    if container.task()? != task.id {
        return None;
    }
    match container {
        ContainerRef::Planner { run, .. } => {
            task.plan_runs.iter().find(|r| r.id == *run)?.herdr.as_ref()
        }
        ContainerRef::PlanReviewer { plan, review, .. } => task
            .plans
            .iter()
            .find(|p| p.id == *plan)?
            .reviews
            .iter()
            .find(|r| r.id == *review)?
            .herdr
            .as_ref(),
        ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. } => {
            container_run(task, container)?.herdr.as_ref()
        }
        ContainerRef::Pm => None,
    }
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

/// The task waits in Needs me only because a question of `parent`'s containers needs the owner.
pub fn question_hold(task: &Task, parent: &Run) -> bool {
    task.status == Status::NeedsMe
        && parent
            .blocked_excerpt
            .as_deref()
            .is_some_and(|e| e.starts_with(super::QUESTION_HOLD_PREFIX))
}

/// F4 closure predicate, pure over one task record. `None` while the container is open.
pub fn container_closure(task: &Task, container: &ContainerRef) -> Option<ClosureReason> {
    closure(task, container, true)
}

/// `verdict_closes: false` asks whether anything other than a recorded verdict closed a review
/// child, so the verdict's own final turn can settle without waiving any other requirement.
fn closure(task: &Task, container: &ContainerRef, verdict_closes: bool) -> Option<ClosureReason> {
    match container {
        ContainerRef::Planner { task: id, run } => (task.id != *id
            || task.status == Status::Dropped
            || task
                .plan_runs
                .last()
                .is_none_or(|r| r.id != *run || !r.holds_slot()))
        .then_some(ClosureReason::RunEnded),
        ContainerRef::PlanReviewer {
            task: id,
            plan,
            review,
        } => {
            if task.id != *id || task.status == Status::Dropped {
                return Some(ClosureReason::RunEnded);
            }
            let Some(parent) = task.plans.last().filter(|p| p.id == *plan) else {
                return Some(ClosureReason::ReviewSuperseded);
            };
            if !crate::plan_contract_revision(task).is_ok_and(|r| r == parent.spec_revision)
                || parent
                    .run
                    .as_ref()
                    .is_some_and(|run| task.plan_runs.last().is_none_or(|r| r.id != *run))
            {
                return Some(ClosureReason::ReviewSuperseded);
            }
            let Some(child) = parent.reviews.last().filter(|r| r.id == *review) else {
                return Some(ClosureReason::ReviewSuperseded);
            };
            (verdict_closes && child.verdict.is_some()).then_some(ClosureReason::ReviewClosed)
        }
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
            if verdict_closes && records().any(|r| r.verdict.is_some()) {
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
    if matches!(
        binding.container,
        ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. }
    ) {
        let state = container_state(task, &binding.container);
        if let Some(reason) = closure(task, &binding.container, !binding.settles_open_turn(state)) {
            return Err(AgentRefusal::new(
                if reason == ClosureReason::RunEnded {
                    RefusalCode::RunEnded
                } else {
                    RefusalCode::ReviewClosed
                },
                reason.as_str(),
            ));
        }
        let state = state.ok_or_else(|| {
            AgentRefusal::new(
                RefusalCode::AgentFenced,
                "the container has no agent binding",
            )
        })?;
        if matches!(binding.container, ContainerRef::PlanReviewer { .. })
            && state.binding.callback != binding.callback
        {
            return Err(AgentRefusal::new(
                RefusalCode::ReviewClosed,
                "the callback token does not match the bound attempt",
            ));
        }
        return generation_live(state, binding.generation);
    }
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
            // The open final turn waives the verdict's closure and nothing else: the parent,
            // last-child, outcome, Review-status, callback and generation rules still apply.
            let settling = binding.settles_open_turn(child_state);
            match closure(task, &binding.container, !settling) {
                None => {}
                Some(ClosureReason::RunEnded) => return Err(ended("the parent run has ended")),
                Some(reason) => return Err(closed(format!("review child {}", reason.as_str()))),
            }
            // A review child's own question may hold the task in Needs me; it stays live then.
            if task.status != Status::Review && !question_hold(task, parent) {
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
