//! Shared read-only responsibility projection for the board and assistant surfaces.
//! Reads only the canonical co-work completion scope and transfer journal. It never
//! infers ownership from origin, labels, process liveness or the latest chat message.
use anyhow::Result;
use corral_core::{CoworkReview, Status, Task, TaskRef};
use serde::{Deserialize, Serialize};

use super::{
    Goal, Journal,
    lifecycle::{self, Disposition, Transfer},
};
use crate::Ctx;

/// Who currently drives the scope. `Reconciling` is shown honestly, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Driver {
    Assistant,
    Board,
    Reconciling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Responsibility {
    pub driver: Driver,
    /// Text label; state is never conveyed by colour alone.
    pub label: String,
    /// The real reason and what resumes it, not a generic "waiting for you".
    pub waiting: Option<String>,
    pub plan_only: bool,
    pub goal: Option<Goal>,
    /// What a continuation started from this display binds; None when none is offered.
    pub pin: Option<super::continuation::Pin>,
}

/// `None` for ordinary board work: no co-work record means no assistant responsibility.
/// A transfer whose journal cannot be read projects `Reconciling` with the named cause.
pub fn project(ctx: &Ctx, task: &Task) -> Option<Responsibility> {
    let cowork = task.cowork.as_ref()?;
    let journal = match lifecycle::for_task(&ctx.store, task) {
        Ok(journal) => journal,
        Err(error) => return Some(reconciling(format!("{error:#}"))),
    };
    let Some(journal) = journal else {
        return Some(cowork_only(task, cowork));
    };
    match transfer_status(ctx, task, &journal) {
        Ok(status) => Some(status),
        Err(error) => Some(reconciling(format!("{error:#}"))),
    }
}

pub(crate) fn reconciling(cause: String) -> Responsibility {
    Responsibility {
        driver: Driver::Reconciling,
        label: "Handoff reconciling".into(),
        waiting: Some(cause),
        plan_only: false,
        goal: None,
        pin: None,
    }
}

fn cowork_only(task: &Task, cowork: &corral_core::Cowork) -> Responsibility {
    // A pre-transfer record is assistant-led; its run binding decides relevance, not origin.
    // No recorded run is not "an earlier run": only a different current run is.
    let applies = task
        .current_run()
        .is_none_or(|run| cowork.applies_to(&run.id));
    match cowork.assistant_driver() {
        Some((session, _)) => {
            let intent = match cowork.completion.as_ref().map(|s| s.intent) {
                _ if !applies => "earlier run",
                Some(corral_core::CompletionIntent::Verify) => "Check only",
                _ if cowork.shipping() => "Ship when checks pass",
                _ => "Manual finish",
            };
            Responsibility {
                driver: Driver::Assistant,
                label: format!("Co-work · {session} · {intent}"),
                waiting: None,
                plan_only: false,
                goal: None,
                pin: None,
            }
        }
        None => Responsibility {
            driver: Driver::Board,
            label: format!("Board finishing · from {}", cowork.session),
            waiting: None,
            plan_only: false,
            goal: None,
            pin: None,
        },
    }
}

fn transfer_status(ctx: &Ctx, task: &Task, journal: &Journal) -> Result<Responsibility> {
    let transfer = journal
        .transfer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("handoff_reconciling: task publication incomplete"))?;
    let plan_only = lifecycle::plan_only(transfer.goal);
    let session = &journal.assistant;
    let (driver, label) = match transfer.disposition {
        Disposition::Saving => (Driver::Reconciling, "Saving for later · Held".to_string()),
        Disposition::Saved if plan_only => (Driver::Board, "Saved plan · Held".into()),
        Disposition::Saved => (Driver::Board, "Saved for later · Held".into()),
        Disposition::Starting => (Driver::Reconciling, "Starting selected goal".into()),
        Disposition::CheckpointingPlan => {
            (Driver::Board, "Board continuing plan · Plan only".into())
        }
        Disposition::Continuing => (Driver::Board, continuing_label(task, transfer)?),
        Disposition::TakingBack => (Driver::Reconciling, format!("Returning to {session}")),
        Disposition::Returned if plan_only => (
            Driver::Assistant,
            format!("Back with {session} · Planning only"),
        ),
        Disposition::Returned => (
            Driver::Assistant,
            format!("Back with {session} · Prepare only"),
        ),
    };
    Ok(Responsibility {
        driver,
        label,
        waiting: journal.waiting.clone().or(waiting(ctx, task, transfer)?),
        plan_only,
        goal: Some(transfer.goal),
        // A refusal here leaves the card without a pin: N/U/T then refuse instead of guessing.
        pin: super::continuation::pin(ctx, &task.id).ok(),
    })
}

fn continuing_label(task: &Task, transfer: &Transfer) -> Result<String> {
    Ok(match transfer.goal {
        Goal::PlanReviewOnly => {
            let reviewed = match &transfer.review_target {
                Some(target) => {
                    task.plans.last().is_some_and(|p| {
                        p.id == target.plan && p.reviews.last().is_some_and(|r| r.verdict.is_some())
                    }) && lifecycle::plan_identity(task)?.findings_sha256 != target.findings_sha256
                }
                None => false,
            };
            match (reviewed, task.plans.last()) {
                (true, _) => "Plan reviewed · Findings ready".into(),
                (false, Some(plan)) => {
                    format!("Board reviewing plan · {} · No implementation", plan.id)
                }
                (false, None) => "Board reviewing plan · No implementation".into(),
            }
        }
        Goal::PlanContinue => "Board continuing plan · Plan only".into(),
        // The selected goal and its stop, never a generic implementation label.
        Goal::ImplementationPrepare => "Board is handling this · Prepare only".into(),
        Goal::ImplementationVerify => "Board is handling this · Check only, never lands".into(),
        Goal::ImplementationShip => "Board is handling this · Ship when checks pass".into(),
    })
}

/// The honest wait for a published transfer. Shared by the CLI readback and both TUIs.
pub(crate) fn waiting(ctx: &Ctx, task: &Task, transfer: &Transfer) -> Result<Option<String>> {
    Ok(match transfer.disposition {
        Disposition::Saving => Some("Stopping scoped writers; board remains held".to_string()),
        Disposition::Saved => Some("Saved for later; explicit continuation required".to_string()),
        Disposition::Starting => {
            Some("Reconciling the selected current-revision continuation".to_string())
        }
        Disposition::CheckpointingPlan => {
            Some("Preserving prior planning context before dispatch".to_string())
        }
        Disposition::TakingBack => Some("Waiting for current operation to settle".to_string()),
        Disposition::Returned => {
            Some("Assistant prepare-only; separate Check/Ship required".to_string())
        }
        Disposition::Continuing if !transfer.ownership_committed || !transfer.adoption_proven => {
            Some("Reconciling held publication/adoption".to_string())
        }
        Disposition::Continuing if transfer.candidate_pending => Some(
            if task.plans.last().is_some_and(|p| p.approval.is_none()) {
                "Waiting for required plan review/acceptance; preserved candidate is not executing"
            } else {
                "Preserved candidate awaiting normal adoption (or an explicit member-scope choice)"
            }
            .into(),
        ),
        Disposition::Continuing if matches!(task.status, Status::Ready | Status::Backlog) => {
            crate::queue::capacity_reason(ctx, &ctx.store, task, false)?
                .or_else(|| Some("Queued for normal admission".into()))
        }
        Disposition::Continuing => None,
    })
}

/// The assistant surface's view of its own scope, from the canonical workflow owner.
/// Ordinary sessions without workflow state stay ordinary: no task, no handoff.
pub fn for_assistant(
    ctx: &Ctx,
    state: Option<(
        corral_core::assistant_workflow::ScopeOwner,
        corral_core::assistant_workflow::WorkflowPhase,
    )>,
) -> Result<serde_json::Value> {
    use corral_core::assistant_workflow::{ScopeOwner, WorkflowPhase};
    let Some((owner, phase)) = state else {
        return Ok(
            serde_json::json!({"driver":"assistant","label":"Ordinary assistant work",
            "task":null,"waiting":null,"plan_only":false,"goal":null,"phase":null}),
        );
    };
    let local = |label: &str, driver: Driver| {
        serde_json::json!({"driver":driver,"label":label,"task":null,"waiting":null,
            "plan_only":false,"goal":null,"phase":phase})
    };
    Ok(match owner {
        ScopeOwner::Board { task } => {
            let current = ctx.store.get(&task)?;
            let status = project(ctx, &current).unwrap_or_else(|| {
                reconciling("handoff_reconciling: board-owned task has no co-work record".into())
            });
            let mut value = serde_json::to_value(&status)?;
            value["task"] = serde_json::json!(task);
            value["phase"] = serde_json::to_value(phase)?;
            // Exact binding inputs for native Save / Work on this here; never a grant.
            value["contract_revision"] = serde_json::json!(current.contract_revision()?);
            let journal = lifecycle::for_task(&ctx.store, &current).ok().flatten();
            value["transfer"] = serde_json::json!(journal.as_ref().map(|j| &j.id));
            value["revision"] = serde_json::json!(
                journal
                    .as_ref()
                    .and_then(|j| j.transfer.as_ref())
                    .map(|t| t.revision)
            );
            value
        }
        ScopeOwner::Local => match phase {
            WorkflowPhase::HandoffPreparing | WorkflowPhase::HandoffReconciling => {
                local("Handoff reconciling", Driver::Reconciling)
            }
            WorkflowPhase::Planning => local("Planning · No implementation", Driver::Assistant),
            WorkflowPhase::Paused => local("Paused · No new actions", Driver::Assistant),
            _ => local("Working together", Driver::Assistant),
        },
    })
}

/// Transfer-fenced records must never be read as an ordinary co-work finish.
pub fn transfer_fenced(task: &Task) -> bool {
    task.cowork
        .as_ref()
        .is_some_and(|c| c.review == CoworkReview::TransferV1)
}
