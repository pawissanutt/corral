//! C3 lifecycle facts live in C2's authoritative journal. The task lookup is only an
//! index into that journal; the workflow remains the canonical scope owner.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, TaskRef, assistant_workflow::ScopeOwner};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Goal, Journal, read_journal};

pub(super) const TASKS: &str = "assistant-handoff-tasks";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Saving,
    Saved,
    Starting,
    Continuing,
    TakingBack,
    Returned,
}

/// Copied, durable provenance, never an evictable owner-input pointer or a global grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Instruction {
    Shell {
        text: String,
        at: Timestamp,
    },
    /// Existing C1 Check/Ship provenance only. It cannot authorize transfer or Dispatch.
    Completion {
        instruction: String,
        via: Option<crate::assistant::owner_action::ViaAssistant>,
        at: Timestamp,
    },
}

impl Instruction {
    /// Domain fence, not authentication. Only the guarded owner entry (or its persisted
    /// recovery selection) can construct a transfer instruction; C1 consent stays C1.
    pub(super) fn require_transfer(&self) -> Result<()> {
        ensure!(
            matches!(self, Self::Shell { .. }),
            "handoff_consent: completion provenance is not authenticated transfer consent"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub contract_revision: String,
    pub goal: Goal,
    pub disposition: Disposition,
    pub instruction: Instruction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewTarget {
    pub plan: String,
    pub sha256: String,
    pub spec_revision: String,
    pub producer_run: String,
    pub producer_profile: String,
    pub review_floor: u64,
    pub findings_sha256: Option<String>,
    pub approval_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub task: String,
    pub revision: u64,
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
    pub history: Vec<Transition>,
    pub candidate_pending: bool,
    pub review_target: Option<ReviewTarget>,
    pub consumed_choices: Vec<String>,
    pub code_pin: Option<super::code_pin::CodePin>,
    pub fresh_seed: bool,
    #[serde(default)]
    pub continuation_seed: Option<super::continuation_seed::Seed>,
    pub draft_predecessor: Option<String>,
    pub imported_plan_run: Option<String>,
    #[serde(default)]
    pub planner_drafts: super::planner_draft::Checkpoints,
}

impl Transfer {
    pub(super) fn record(&mut self, instruction: Instruction) -> Result<()> {
        ensure!(
            self.history.len() < 64,
            "handoff_history_full: preserve/reconcile transfer history before another instruction"
        );
        self.history.push(Transition {
            contract_revision: self.contract_revision.clone(),
            goal: self.goal,
            disposition: self.disposition,
            instruction,
        });
        Ok(())
    }

    pub(super) fn advance(&mut self) -> Result<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .context("transfer revision overflow")?;
        Ok(())
    }

    pub(super) fn instructed_at(&self) -> Timestamp {
        let instruction = self
            .history
            .last()
            .map_or(&self.instruction, |t| &t.instruction);
        match instruction {
            Instruction::Shell { at, .. } | Instruction::Completion { at, .. } => *at,
        }
    }
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
    let owned = workflow.owner
        == ScopeOwner::Board {
            task: transfer.task.clone(),
        };
    if owned {
        ensure!(
            transfer.ownership_committed,
            "handoff_reconciling: owner publication incomplete"
        );
        journal
            .source_boundary
            .as_ref()
            .context("handoff_source_unsupported: published transfer has no source drain proof")?
            .require_published(&container, journal)?;
    }
    Ok(owned)
}

/// Review requires fresh scope; renewed planning may retain an older exact plan as
/// context. Identity pinning itself must not silently impose the review-only goal.
pub(super) fn review_target<T: TaskRef + ?Sized>(task: &T) -> Result<ReviewTarget> {
    let target = plan_identity(task)?;
    ensure!(
        target.spec_revision == crate::plan_spec_revision(task)?,
        "plan_review_only: plan/spec changed; refresh"
    );
    Ok(target)
}

pub(super) fn plan_identity<T: TaskRef + ?Sized>(task: &T) -> Result<ReviewTarget> {
    let plan = task
        .last_plan_head()?
        .context("handoff_plan: no formal plan to pin")?;
    let run = plan
        .run
        .as_deref()
        .context("handoff_plan: exact plan producer is unnamed")?;
    let producer = task.plan_run_head(run)?.context(
        "handoff_plan: exact plan producer is unavailable; do not relabel it as another producer",
    )?;
    Ok(ReviewTarget {
        plan: plan.id.clone(),
        sha256: plan.artifact.sha256.clone(),
        spec_revision: plan.spec_revision.clone(),
        producer_run: producer.id.clone(),
        producer_profile: producer.profile.clone(),
        review_floor: plan.facts.children,
        findings_sha256: plan
            .current_review
            .as_ref()
            .map(|r| {
                serde_json::to_vec(&(
                    &r.id,
                    &r.reviewer,
                    &r.policy_digest,
                    r.verdict,
                    &r.notes,
                    r.cancelled,
                ))
            })
            .transpose()?
            .map(|bytes| crate::verification::digest(&bytes)),
        approval_sha256: plan
            .approval
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()?
            .map(|bytes| crate::verification::digest(&bytes)),
    })
}

pub(crate) fn plan_only(goal: Goal) -> bool {
    matches!(goal, Goal::PlanContinue | Goal::PlanReviewOnly)
}
