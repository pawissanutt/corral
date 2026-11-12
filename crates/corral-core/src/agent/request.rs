//! A PM or assistant session's request ledger around its todo plan (T-679): plan identity and
//! rollover history, the disposition each turn ended with, and bounded continuation. It lives
//! beside `AgentRunState.todo` in the session's own container; worker runs keep run progress.
use anyhow::{Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{TodoAction, TurnOrigin, TurnOriginKind};
use crate::{RunProgress, StepState, WaitOn};

/// Closed plans a session keeps, oldest evicted first.
pub const PLAN_HISTORY_MAX: usize = 16;
/// Automatic continuations of one plan in a row; a turn the owner or corral starts resets it.
pub const CONTINUATIONS_MAX: u32 = 8;

/// Shared lease for session-ledger deadline mutations and PM wait publication.
pub const WAIT_PUBLICATION_LOCK: &str = "pm-wait-wake-outbox";

/// Hold the wait-publication lease through the caller's existing container transaction.
/// Acquire it before entering that transaction, preserving the lease-before-dispatch order.
pub fn with_wait_publication_lock<T>(
    store: &crate::Store,
    apply: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _lease = store.lock(WAIT_PUBLICATION_LOCK)?;
    apply()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestLedger {
    /// Plans opened in this container; the current plan is `P<plans>`.
    #[serde(default)]
    pub plans: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<PlanRecord>,
    /// How the latest turn ended for the current request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<TurnDisposition>,
    /// The latest continuation corral granted, and whether it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continued: Option<Continued>,
}

/// A completed or dropped plan, retained after a later plan replaced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    pub plan: String,
    pub revision: u64,
    pub steps: usize,
    pub done: usize,
    pub dropped: usize,
    pub closed: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Disposition {
    Complete,
    WaitingEvent {
        event: String,
        steps: Vec<String>,
    },
    AwaitingOwner {
        question: String,
        steps: Vec<String>,
    },
    Actionable {
        next: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEnd {
    Normal,
    /// The host still owns a managed command; its completion wake resumes the session.
    WaitingCommand,
    Cancelled,
    ProviderFailed,
    BudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    NoProgress,
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Continuation {
    /// Nothing actionable remains, or every open step waits on an event or the owner.
    NotDue,
    /// The turn was cancelled or failed: it is never continued automatically.
    Held,
    Due {
        key: String,
        attempt: u32,
        max: u32,
    },
    /// This plan revision already had its continuation.
    Duplicate {
        key: String,
    },
    Stopped {
        reason: StopReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDisposition {
    pub generation: u32,
    pub turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub revision: u64,
    pub end: TurnEnd,
    pub disposition: Disposition,
    pub continuation: Continuation,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Continued {
    /// The turn origin key the continuation's turn reports.
    pub key: String,
    pub plan: String,
    pub revision: u64,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StopReason>,
}

fn plan_id(n: u64) -> String {
    format!("P{n}")
}

fn open(state: &StepState) -> bool {
    matches!(state, StepState::Pending | StepState::InProgress)
}

/// What remains of a plan: the next step nothing waits on, else what the open steps wait on.
/// An owner question blocks only the steps that wait on it.
pub fn disposition(progress: Option<&RunProgress>) -> Disposition {
    let Some(progress) = progress else {
        return Disposition::Complete;
    };
    let waits = |id: &str| progress.waits.iter().find(|w| w.step == id);
    let free = |state: StepState| {
        progress
            .steps()
            .find(|s| s.status == state && waits(&s.id).is_none())
    };
    if let Some(next) = free(StepState::InProgress).or_else(|| free(StepState::Pending)) {
        return Disposition::Actionable {
            next: next.id.clone(),
        };
    }
    let waiting: Vec<_> = progress
        .steps()
        .filter(|s| open(&s.status))
        .filter_map(|s| waits(&s.id))
        .collect();
    let owner: Vec<_> = waiting
        .iter()
        .filter(|w| matches!(w.on, WaitOn::Owner { .. }))
        .collect();
    match (owner.first(), waiting.first()) {
        (Some(first), _) => Disposition::AwaitingOwner {
            question: match &first.on {
                WaitOn::Owner { question } => question.clone(),
                WaitOn::Event { event } => event.clone(),
            },
            steps: owner.iter().map(|w| w.step.clone()).collect(),
        },
        (None, Some(first)) => Disposition::WaitingEvent {
            event: match &first.on {
                WaitOn::Event { event } => event.clone(),
                WaitOn::Owner { question } => question.clone(),
            },
            steps: waiting.iter().map(|w| w.step.clone()).collect(),
        },
        (None, None) => Disposition::Complete,
    }
}

impl RequestLedger {
    /// The current plan's identity; a ledger written before plan identities is plan P1.
    pub fn current_plan(&self, todo: Option<&RunProgress>) -> Option<String> {
        todo.map(|p| p.plan.clone().unwrap_or_else(|| plan_id(self.plans.max(1))))
    }

    /// Apply one todo command to the session's plan. An init rolls a plan whose steps are all
    /// done or dropped into history; it never replaces a plan with open steps.
    pub fn apply(
        &mut self,
        action: &TodoAction,
        todo: Option<&RunProgress>,
        spec: &str,
        now: Timestamp,
    ) -> Result<RunProgress> {
        if let Some(previous) = todo {
            ensure!(previous.spec_revision == spec, "stale_todo_spec");
        }
        let current = self.current_plan(todo);
        action.check_plan(current.as_deref())?;
        let plans = self.plans.max(u64::from(todo.is_some()));
        let TodoAction::Init { .. } = action else {
            let mut progress = action.step(todo, spec, 0, Some(now))?;
            progress.plan = current;
            self.plans = plans;
            return Ok(progress);
        };
        let mut floor = 0;
        if let (Some(previous), Some(plan)) = (todo, current) {
            ensure!(!previous.steps().any(|s| open(&s.status)), "todo_plan_open");
            let count =
                |f: fn(&StepState) -> bool| previous.steps().filter(|s| f(&s.status)).count();
            self.history.push(PlanRecord {
                plan,
                revision: previous.revision,
                steps: previous.steps().count(),
                done: count(|s| *s == StepState::Done),
                dropped: count(|s| matches!(s, StepState::Dropped { .. })),
                closed: now,
            });
            let excess = self.history.len().saturating_sub(PLAN_HISTORY_MAX);
            self.history.drain(..excess);
            floor = previous.revision;
        }
        let mut progress = action.step(None, spec, floor, Some(now))?;
        self.plans = plans + 1;
        progress.plan = Some(plan_id(self.plans));
        Ok(progress)
    }

    /// Records how a turn ended for the current request and whether corral continues it.
    pub fn turn_ended(
        &mut self,
        todo: Option<&RunProgress>,
        end: TurnEnd,
        origin: &TurnOrigin,
        (generation, turn): (u32, u64),
        now: Timestamp,
    ) -> TurnDisposition {
        let disposition = disposition(todo);
        let plan = self.current_plan(todo);
        let revision = todo.map_or(0, |p| p.revision);
        let continuation = match (end, &disposition, &plan) {
            (TurnEnd::Normal, Disposition::Actionable { .. }, Some(plan)) => {
                self.continuation(plan, revision, origin)
            }
            (TurnEnd::Normal, _, _) => Continuation::NotDue,
            _ => Continuation::Held,
        };
        let record = TurnDisposition {
            generation,
            turn,
            plan,
            revision,
            end,
            disposition,
            continuation,
            at: now,
        };
        self.turn = Some(record.clone());
        record
    }

    /// One continuation per plan revision, `CONTINUATIONS_MAX` in a row, and none after a
    /// continued turn that left the plan revision unchanged.
    fn continuation(&mut self, plan: &str, revision: u64, origin: &TurnOrigin) -> Continuation {
        let previous = self.continued.as_mut().filter(|c| c.plan == plan);
        let continuing = previous.as_ref().is_some_and(|c| {
            origin.kind == TurnOriginKind::Continuation
                && origin.idempotency_key.as_deref() == Some(c.key.as_str())
        });
        if let Some(same) = previous.filter(|c| c.revision == revision) {
            if let Some(reason) = same.stopped {
                return Continuation::Stopped { reason };
            }
            if continuing {
                same.stopped = Some(StopReason::NoProgress);
                return Continuation::Stopped {
                    reason: StopReason::NoProgress,
                };
            }
            return Continuation::Duplicate {
                key: same.key.clone(),
            };
        }
        let attempt = match &self.continued {
            Some(c) if continuing => c.attempt + 1,
            _ => 1,
        };
        let key = format!("continue:{plan}:{revision}");
        let stopped = (attempt > CONTINUATIONS_MAX).then_some(StopReason::Limit);
        self.continued = Some(Continued {
            key: key.clone(),
            plan: plan.into(),
            revision,
            attempt: attempt.min(CONTINUATIONS_MAX),
            stopped,
        });
        match stopped {
            Some(reason) => Continuation::Stopped { reason },
            None => Continuation::Due {
                key,
                attempt,
                max: CONTINUATIONS_MAX,
            },
        }
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
