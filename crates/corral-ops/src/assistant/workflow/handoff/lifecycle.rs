//! C3 lifecycle facts live in C2's authoritative journal. The task lookup is only an
//! index into that journal; the workflow remains the canonical scope owner.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, TaskRef, assistant::OwnerInput, assistant_workflow::ScopeOwner};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Goal, Journal, read_journal};

pub(super) const TASKS: &str = "assistant-handoff-tasks";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Saved,
    Continuing,
    TakingBack,
    Returned,
}

/// Copied, durable provenance, never an evictable owner-input pointer or a global grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Instruction {
    Assistant { input: OwnerInput, request: String },
    Shell { text: String, at: Timestamp },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub task: String,
    pub contract_revision: String,
    pub goal: Goal,
    pub disposition: Disposition,
    pub instruction: Instruction,
    pub ownership_committed: bool,
    pub notification_pending: bool,
    pub adoption_proven: bool,
    pub seed_run: Option<String>,
    pub return_snapshot: Option<super::Snapshot>,
    pub returned_scratch: Option<String>,
    pub renewed_assistant: bool,
    pub choice_digest: String,
}

pub fn for_task<T: TaskRef + ?Sized>(store: &Store, task: &T) -> Result<Option<Journal>> {
    let id: Option<String> = store.record(TASKS, task.id())?;
    let fenced = task
        .cowork()
        .as_ref()
        .is_some_and(|c| c.review == corral_core::CoworkReview::TransferV1);
    let Some(id) = id else {
        ensure!(
            !fenced,
            "handoff_reconciling: transfer task lost its journal binding"
        );
        return Ok(None);
    };
    let journal =
        read_journal(store, &id)?.context("handoff_reconciling: missing transfer journal")?;
    let transfer = journal
        .transfer
        .as_ref()
        .context("handoff_reconciling: task publication incomplete")?;
    ensure!(
        fenced && transfer.task == *task.id(),
        "handoff_reconciling: task/journal identity differs"
    );
    Ok(Some(journal))
}

pub(crate) fn board_owner(store: &Store, journal: &Journal) -> Result<bool> {
    let transfer = journal
        .transfer
        .as_ref()
        .context("handoff_reconciling: no task")?;
    let container = store.get_assistant_container(&journal.assistant)?;
    let workflow = super::super::current(&container)?;
    ensure!(
        workflow.id == journal.expected.workflow
            && workflow
                .handoff
                .as_ref()
                .is_some_and(|a| a.blob == journal.package.blob),
        "handoff_reconciling: workflow no longer binds this transfer"
    );
    Ok(workflow.owner
        == ScopeOwner::Board {
            task: transfer.task.clone(),
        })
}

pub(crate) fn plan_only(goal: Goal) -> bool {
    matches!(goal, Goal::PlanContinue | Goal::PlanReviewOnly)
}
