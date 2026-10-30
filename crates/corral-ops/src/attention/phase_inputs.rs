//! History-free phase inputs. Cold adapters inspect the caller's actual draft; head adapters
//! read only current metadata and active reviewer rows. These are not Task/Run facades.
use anyhow::Result;
use corral_core::agent::HostHealth;
use corral_core::{
    AggregateReview, AutopilotState, GateRecord, HerdrRef, PlanApproval, PlanRunState,
    ReviewRecord, RunOutcome, Task, TaskHead, Verdict,
};
use jiff::Timestamp;

#[cfg(test)]
#[path = "phase_inputs_tests.rs"]
mod tests;

#[derive(Debug, PartialEq)]
pub(super) struct ReviewerPhase {
    pub started: Timestamp,
    pub agent_state: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(super) struct RunPhase {
    pub id: String,
    pub started: Timestamp,
    pub outcome: Option<RunOutcome>,
    pub agent_state: Option<String>,
    pub idle_no_report: bool,
    pub autopilot: Option<AutopilotState>,
    pub gate: Option<GateRecord>,
    pub review: Option<ReviewRecord>,
    pub latest_reviewer: Option<ReviewerPhase>,
    pub lost: bool,
}

macro_rules! run_phase {
    ($run:expr,$reviewer:expr) => {{
        let run = $run;
        RunPhase {
            id: run.id.clone(),
            started: run.started,
            outcome: run.outcome,
            agent_state: run.agent_state.clone(),
            idle_no_report: run.idle_no_report,
            autopilot: run.autopilot.clone(),
            gate: run.gate.clone(),
            review: run.review.clone(),
            latest_reviewer: $reviewer,
            lost: run.agent_state.as_deref() == Some("gone")
                || run.agent_host.as_ref().is_some_and(|host| {
                    matches!(host.binding.health, HostHealth::Lost | HostHealth::Fenced)
                }),
        }
    }};
}

#[derive(Debug, PartialEq)]
pub(super) struct PlanningPhase {
    pub id: String,
    pub state: PlanRunState,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub submitted: Option<String>,
    pub holds_slot: bool,
}

#[derive(Debug, PartialEq)]
pub(super) struct PlanReviewPhase {
    pub id: String,
    pub at: Timestamp,
    pub verdict: Option<Verdict>,
    pub herdr: Option<HerdrRef>,
}

#[derive(Debug, PartialEq)]
pub(super) struct PlanPhase {
    pub id: String,
    pub run: Option<String>,
    pub approval: Option<PlanApproval>,
    pub added: Timestamp,
    pub current_review: Option<PlanReviewPhase>,
}

#[derive(Debug, PartialEq)]
pub(super) struct AggregatePhase {
    pub id: String,
    pub started: Timestamp,
    pub gate: Option<GateRecord>,
    pub current_review: Option<AggregateReview>,
}

#[derive(Debug, PartialEq, Default)]
pub(super) struct Phases {
    pub run: Option<RunPhase>,
    pub planning: Option<PlanningPhase>,
    pub plan: Option<PlanPhase>,
    pub aggregate: Option<AggregatePhase>,
    pub dispatch_credit: bool,
}

macro_rules! planning {
    ($run:expr) => {{
        let run = $run;
        PlanningPhase {
            id: run.id.clone(),
            state: run.state,
            started: run.started,
            ended: run.ended,
            submitted: run.submitted.clone(),
            holds_slot: run.holds_slot(),
        }
    }};
}
macro_rules! plan_review {
    ($review:expr) => {{
        let review = $review;
        PlanReviewPhase {
            id: review.id.clone(),
            at: review.at,
            verdict: review.verdict,
            herdr: review.herdr.clone(),
        }
    }};
}

impl Phases {
    pub fn full(task: &Task) -> Self {
        let run = task.current_run().map(|run| {
            let reviewer = run
                .review_runs
                .iter()
                .rev()
                .find(|child| child.ended.is_none() && child.outcome.is_none())
                .map(|child| ReviewerPhase {
                    started: child.started,
                    agent_state: child.agent_state.clone(),
                });
            run_phase!(run, reviewer)
        });
        Self {
            run,
            planning: task.plan_runs.last().map(|run| planning!(run)),
            plan: task.plans.last().map(|plan| PlanPhase {
                id: plan.id.clone(),
                run: plan.run.clone(),
                approval: plan.approval.clone(),
                added: plan.artifact.added,
                current_review: plan.reviews.last().map(|review| plan_review!(review)),
            }),
            aggregate: task
                .aggregate_attempts
                .last()
                .map(|attempt| AggregatePhase {
                    id: attempt.id.clone(),
                    started: attempt.started,
                    gate: attempt.gate.clone(),
                    current_review: attempt.reviews.last().cloned(),
                }),
            dispatch_credit: task.holds_dispatch_slot(),
        }
    }

    pub fn head(task: &TaskHead) -> Result<Self> {
        let run = task
            .current_run
            .as_deref()
            .map(|run| -> Result<RunPhase> {
                let reviewer = task
                    .open_review_children(&run.id)?
                    .into_iter()
                    .rev()
                    .find(|child| child.ended.is_none() && child.outcome.is_none())
                    .map(|child| ReviewerPhase {
                        started: child.started,
                        agent_state: child.agent_state,
                    });
                Ok(run_phase!(run, reviewer))
            })
            .transpose()?;
        Ok(Self {
            run,
            planning: task.last_plan_run.as_deref().map(|run| planning!(run)),
            plan: task.last_plan.as_deref().map(|plan| PlanPhase {
                id: plan.id.clone(),
                run: plan.run.clone(),
                approval: plan.approval.clone(),
                added: plan.artifact.added,
                current_review: plan
                    .current_review
                    .as_ref()
                    .map(|review| plan_review!(review)),
            }),
            aggregate: task
                .last_aggregate_attempt
                .as_deref()
                .map(|attempt| AggregatePhase {
                    id: attempt.id.clone(),
                    started: attempt.started,
                    gate: attempt.gate.clone(),
                    current_review: attempt.current_review.clone(),
                }),
            dispatch_credit: task.holds_dispatch_slot(),
        })
    }
}
