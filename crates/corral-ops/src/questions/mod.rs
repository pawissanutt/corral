//! Pi-native agent questions (T-252 question-lifecycle). An AgentQuestion lives on the asking
//! worker run's or review child's `AgentRunState`, keyed `<task>/<run>[/<review-child>]/Q<n>`.
//! The asking agent opens and withdraws it through `corral agent call`; the PM and the owner
//! triage it by key and revision; `terminalize`, `expire_deadlines` and `unattended` are the
//! sweeps the sync hook (integration-seams) calls. Readers apply the F4 container closure, so a
//! question of a closed container is non-pending before `terminalize` persists it.
mod ask;
mod basis;
mod events;
mod lifecycle;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_durability;
#[cfg(test)]
mod tests_lifecycle;
#[cfg(test)]
mod tests_support;
mod triage;

use anyhow::Result;
use corral_core::Task;
use corral_core::agent::{
    self, AgentQuestion, AgentRunState, ClosureReason, ContainerRef, QUESTION_TRIAGE_MAX,
    QuestionActor, QuestionState, QuestionTriage,
};
use corral_core::inbox::Recipient;
use jiff::Timestamp;
use serde::Serialize;

pub use ask::{
    AskPayload, NOTE_MAX_BYTES, OPTION_MAX_BYTES, Opened, REF_MAX_BYTES, WithdrawPayload, open,
    validate_ask, wake_route, withdraw,
};
pub use events::{QUESTION_EVENT_KINDS, reconcile_events, reconcile_question};
pub use lifecycle::{expire_deadlines, terminalize, unattended};
pub use triage::{
    Actor, AnswerInput, Applied, Outcome, PmRequest, answer, cancel, deadline, escalate,
};
pub use triage::{hold_owner_attention, settle_needs_me};

use crate::Ctx;

/// Inbox message kinds of the Pi-native adapter that a closed container never receives.
pub const PI_NATIVE_MESSAGE_KINDS: [&str; 7] = [
    "question_answer",
    "question_correction",
    "turn_nudge",
    "wait_wake",
    "question_wake",
    "recovery_notice",
    "recovery_update",
];

/// Why a question operation was refused; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    NotFound,
    /// The asking run or review child is closed, or the question was terminalized.
    Closed,
    Withdrawn,
    StaleRevision,
    /// The transition does not apply to the question's current state.
    State,
    /// OD7: PM answer authority, basis refs and upward-only reclassification.
    Rule,
    /// At most one blocking and eight pending questions per container.
    Limit,
    /// Text, option or reference bounds; overflow is refused, never truncated.
    Bounds,
    OutOfScope,
    PayloadMismatch,
}

impl RefusalKind {
    pub fn code(self) -> &'static str {
        match self {
            RefusalKind::NotFound => "question_not_found",
            RefusalKind::Closed => "question_closed",
            RefusalKind::Withdrawn => "question_withdrawn",
            RefusalKind::StaleRevision => "stale_revision",
            RefusalKind::State => "question_state",
            RefusalKind::Rule => "question_rule",
            RefusalKind::Limit => "question_limit",
            RefusalKind::Bounds => "bad_request",
            RefusalKind::OutOfScope => "out_of_scope",
            RefusalKind::PayloadMismatch => "payload_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionRefusal {
    pub kind: RefusalKind,
    pub reason: String,
}

impl QuestionRefusal {
    pub fn new(kind: RefusalKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for QuestionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.code(), self.reason)
    }
}

impl std::error::Error for QuestionRefusal {}

pub(crate) fn refuse(kind: RefusalKind, reason: impl Into<String>) -> anyhow::Error {
    QuestionRefusal::new(kind, reason).into()
}

/// A parsed question key: its container and sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionKey {
    pub container: ContainerRef,
    pub n: u32,
}

impl QuestionKey {
    pub fn parse(key: &str) -> Result<Self, QuestionRefusal> {
        let bad = || {
            QuestionRefusal::new(
                RefusalKind::NotFound,
                format!("{key:?} is not a question key <task>/<run>[/<review-child>]/Q<n>"),
            )
        };
        let parts: Vec<&str> = key.split('/').collect();
        let n = parts
            .last()
            .and_then(|last| last.strip_prefix('Q'))
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| *n > 0)
            .ok_or_else(bad)?;
        let word = |part: &&str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        if !parts[..parts.len() - 1].iter().all(word) {
            return Err(bad());
        }
        // Planning containers key their run or plan as `plan-<D|P>` (`ContainerRef::key`).
        let container = match parts.as_slice() {
            [task, run, _] => match run.strip_prefix("plan-") {
                Some(run) => ContainerRef::Planner {
                    task: (*task).into(),
                    run: run.into(),
                },
                None => ContainerRef::Worker {
                    task: (*task).into(),
                    run: (*run).into(),
                },
            },
            [task, run, review, _] => match run.strip_prefix("plan-") {
                Some(plan) => ContainerRef::PlanReviewer {
                    task: (*task).into(),
                    plan: plan.into(),
                    review: (*review).into(),
                },
                None => ContainerRef::Reviewer {
                    task: (*task).into(),
                    run: (*run).into(),
                    review: (*review).into(),
                },
            },
            _ => return Err(bad()),
        };
        Ok(Self { container, n })
    }

    pub fn task(&self) -> &str {
        self.container.task().unwrap_or_default()
    }
}

/// The key a container gives its n-th question.
pub fn question_key(container: &ContainerRef, n: usize) -> String {
    format!("{}/Q{n}", container.key())
}

/// Every Pi-native worker run, review child, planner run and plan review of a task, current or
/// historical.
pub fn containers(task: &Task) -> Vec<ContainerRef> {
    let mut out = crate::agent::ops::planning::containers(task);
    for run in &task.runs {
        if run.agent_host.is_some() {
            out.push(ContainerRef::Worker {
                task: task.id.clone(),
                run: run.id.clone(),
            });
        }
        for child in run.review_runs.iter().filter(|c| c.agent_host.is_some()) {
            out.push(ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            });
        }
    }
    out
}

/// A closed container whose run-end duties sync already settled (`InboxState.settled`): its
/// questions, events and messages are final, so no sweep visits it again.
pub(crate) fn settled(task: &Task, container: &ContainerRef) -> bool {
    agent::container_run(task, container).is_some_and(|run| run.inbox.settled.is_some())
}

/// Pending for readers: recorded open or escalated, not terminalized, and its container open.
pub fn is_pending(task: &Task, container: &ContainerRef, question: &AgentQuestion) -> bool {
    question.recorded_pending() && agent::container_closure(task, container).is_none()
}

/// A pending question that waits on the owner: escalated, or marked unattended at its current
/// revision.
pub fn needs_owner(question: &AgentQuestion) -> bool {
    question.state == QuestionState::Escalated || unattended_marked(question)
}

/// `unattended_at` is the current revision's marker: every revision transition clears it, so it
/// never depends on the evictable triage tail.
pub(crate) fn unattended_marked(question: &AgentQuestion) -> bool {
    question.unattended_at.is_some()
}

/// What readers see: the record plus its effective state after the F4 closure.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionView {
    pub task: String,
    pub container: String,
    pub effective_state: QuestionState,
    pub pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closure: Option<ClosureReason>,
    pub question: AgentQuestion,
}

pub fn views(task: &Task) -> Vec<QuestionView> {
    let mut out = Vec::new();
    for container in containers(task) {
        let closure = agent::container_closure(task, &container);
        let Some(state) = agent::container_state(task, &container) else {
            continue;
        };
        for question in &state.questions {
            let pending = question.recorded_pending() && closure.is_none();
            let effective_state = if question.recorded_pending() && closure.is_some() {
                QuestionState::Cancelled
            } else {
                question.state
            };
            out.push(QuestionView {
                task: task.id.clone(),
                container: container.key(),
                effective_state,
                pending,
                closure: closure
                    .filter(|_| question.recorded_pending() || question.terminal.is_some()),
                question: question.clone(),
            });
        }
    }
    out
}

/// Questions of one task or every task; `all` includes non-pending ones.
pub fn list(ctx: &Ctx, task: Option<&str>, all: bool) -> Result<Vec<QuestionView>> {
    let tasks = match task {
        Some(id) => vec![ctx.store.get(id)?],
        None => ctx.store.list()?,
    };
    Ok(tasks
        .iter()
        .flat_map(views)
        .filter(|view| all || view.pending)
        .collect())
}

pub fn show(ctx: &Ctx, key: &str) -> Result<QuestionView> {
    let parsed = QuestionKey::parse(key)?;
    let task = ctx.store.get(parsed.task())?;
    views(&task)
        .into_iter()
        .find(|view| view.question.key == key)
        .ok_or_else(|| refuse(RefusalKind::NotFound, format!("question {key} not found")))
}

pub(crate) fn find_mut<'a>(
    state: &'a mut AgentRunState,
    key: &str,
) -> Option<&'a mut AgentQuestion> {
    state.questions.iter_mut().find(|q| q.key == key)
}

/// Appends a triage entry; `bound_triage` enforces `QUESTION_TRIAGE_MAX` before the commit.
pub(crate) fn triage(
    question: &mut AgentQuestion,
    action: &str,
    by: QuestionActor,
    note: Option<String>,
    at: Timestamp,
) {
    question.triage.push(QuestionTriage {
        action: action.into(),
        by,
        revision: question.revision,
        note,
        at,
    });
}

/// Evicts triage beyond `QUESTION_TRIAGE_MAX`, oldest first, after logging any of the evicted
/// entries' events still owed: each belongs to an earlier, committed transition, and the entry
/// is its only durable evidence. A failed write aborts the caller's transaction, so the
/// evidence is kept.
pub(crate) fn bound_triage(store: &corral_core::Store, question: &mut AgentQuestion) -> Result<()> {
    let excess = question.triage.len().saturating_sub(QUESTION_TRIAGE_MAX);
    if excess > 0 {
        events::drain(store, question, &question.triage[..excess])?;
        question.triage.drain(..excess);
    }
    Ok(())
}

/// Every transition bumps the revision before its triage entry is written.
pub(crate) fn transition(
    question: &mut AgentQuestion,
    state: QuestionState,
    action: &str,
    by: QuestionActor,
    note: Option<String>,
    at: Timestamp,
) {
    question.revision += 1;
    question.state = state;
    question.unattended_at = None;
    triage(question, action, by, note, at);
}

/// The inbox recipient of a container's messages: the worker run, or the exact review child.
pub(crate) fn recipient(container: &ContainerRef) -> Option<Recipient> {
    match container {
        ContainerRef::Planner { task, run } => Some(Recipient::PlanRun {
            task: task.clone(),
            plan_run: run.clone(),
        }),
        ContainerRef::PlanReviewer { task, plan, review } => Some(Recipient::PlanReview {
            task: task.clone(),
            plan: plan.clone(),
            review: review.clone(),
        }),
        ContainerRef::Worker { task, run } => Some(Recipient::Run {
            task: task.clone(),
            run: run.clone(),
        }),
        ContainerRef::Reviewer { task, run, review } => Some(Recipient::ReviewRun {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
        }),
        ContainerRef::Pm => None,
    }
}

pub(crate) fn bounded(value: &str, max: usize, what: &str) -> Result<(), QuestionRefusal> {
    if value.trim().is_empty() {
        return Err(QuestionRefusal::new(
            RefusalKind::Bounds,
            format!("{what} is empty"),
        ));
    }
    if value.len() > max {
        return Err(QuestionRefusal::new(
            RefusalKind::Bounds,
            format!("{what} is {} bytes; the bound is {max}", value.len()),
        ));
    }
    Ok(())
}
