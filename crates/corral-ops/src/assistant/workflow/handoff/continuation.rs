//! A saved hold and a plan-only goal cannot be released by enqueue, acceptance or a
//! workspace grant. A new, exact current-revision instruction changes that disposition.
use super::{
    Goal,
    lifecycle::{self, Disposition, Instruction, ReviewTarget},
};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{CompletionIntent, Status, Task, TaskRef};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub transfer: String,
    pub task: String,
    pub revision: u64,
    pub contract_revision: String,
    pub goal: Goal,
    pub plan: Option<ReviewTarget>,
    pub code: Option<super::code_pin::CodePin>,
}

impl Choice {
    pub fn digest(&self) -> Result<String> {
        Ok(crate::verification::digest(&serde_json::to_vec(self)?))
    }
}

pub fn preview(ctx: &Ctx, id: &str, goal: Goal) -> Result<Choice> {
    let task = ctx.store.head(id)?;
    let journal =
        lifecycle::for_task(&ctx.store, &task)?.context("handoff: task has no transfer")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    ensure!(
        transfer.ownership_committed && lifecycle::board_owner(&ctx.store, &journal)?,
        "handoff_owner: transfer the current assistant work instead of restarting an old board checkpoint"
    );
    ensure!(
        matches!(
            transfer.disposition,
            Disposition::Saved | Disposition::Continuing | Disposition::Starting
        ),
        "handoff: take-back is pending; settle it first"
    );
    ensure!(
        !matches!(task.status, Status::Done | Status::Dropped),
        "handoff: task is terminal"
    );
    let plan = task
        .last_plan_head()?
        .map(|_| {
            if goal == Goal::PlanReviewOnly {
                lifecycle::review_target(&task)
            } else {
                lifecycle::plan_identity(&task)
            }
        })
        .transpose()?;
    ensure!(
        goal != Goal::PlanReviewOnly || plan.is_some(),
        "plan_review_only: formal reviewable snapshot and actual producer required; no replacement planner"
    );
    let contract_revision = task.contract_revision()?;
    let code = super::code_pin::current(ctx, &task)?;
    Ok(Choice {
        transfer: journal.id.clone(),
        task: task.id.clone(),
        revision: transfer.revision,
        contract_revision,
        goal,
        plan,
        code,
    })
}

pub(crate) fn start(
    ctx: &Ctx,
    choice: &Choice,
    digest: &str,
    instruction: Instruction,
) -> Result<Task> {
    instruction.require_transfer()?;
    ensure!(
        choice.digest()? == digest,
        "handoff_stale_choice: selected continuation changed"
    );
    super::read_package(&ctx.store, &choice.transfer)?;
    let _pipeline = ctx
        .store
        .try_lock(&format!("autopilot-{}", choice.task))?
        .context("handoff: current controller/landing must settle")?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{}", choice.task))?
        .context("handoff: launch must settle")?;
    let _gate = ctx
        .store
        .try_lock(&format!("gate-{}", choice.task))?
        .context("handoff: gate admission must settle")?;
    let task = ctx.store.update(&choice.task, |task| {
        let mut journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
        ensure!(
            journal.id == choice.transfer && lifecycle::board_owner(&ctx.store, &journal)?,
            "handoff_owner: current transfer changed"
        );
        let transfer = journal.transfer.as_ref().context("transfer missing")?;
        if transfer.consumed_choices.iter().any(|old| old == digest) {
            return Ok(()); // Durable replay, including after a later Save; never re-release a hold.
        }
        ensure!(
            transfer.revision == choice.revision
                && task.contract_revision()? == choice.contract_revision,
            "handoff_stale_choice: task/spec/disposition changed; refresh"
        );
        let actual_plan = task
            .last_plan_head()?
            .map(|_| {
                if choice.goal == Goal::PlanReviewOnly {
                    lifecycle::review_target(task)
                } else {
                    lifecycle::plan_identity(task)
                }
            })
            .transpose()?;
        ensure!(
            actual_plan == choice.plan,
            "handoff_stale_choice: plan, producer or findings changed; refresh"
        );
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        ensure!(
            matches!(
                transfer.disposition,
                Disposition::Saved | Disposition::Continuing | Disposition::Starting
            ),
            "handoff: take-back must settle first"
        );
        if lifecycle::plan_only(choice.goal) {
            ensure!(
                task.current_run().is_none_or(|r| r.outcome.is_some()
                    && r.gate
                        .as_ref()
                        .is_none_or(|g| g.result != corral_core::GateResult::Running)),
                "handoff_busy: settle existing implementation before selecting a plan-only goal"
            );
        }
        if !lifecycle::plan_only(choice.goal)
            && lifecycle::plan_only(transfer.goal)
            && task.current_run().is_none()
        {
            // Existing owned code remains canonical across plan-only stops. Replaying
            // the original package here would duplicate a run or rewind later work.
            transfer.adoption_proven = false;
        }
        transfer.record(instruction.clone())?;
        transfer.advance()?;
        transfer.consumed_choices.push(digest.into());
        transfer.code_pin = choice.code.clone();
        transfer.goal = choice.goal;
        transfer.disposition = Disposition::Starting;
        transfer.choice_digest = digest.into();
        transfer.contract_revision = choice.contract_revision.clone();
        transfer.review_target = (choice.goal == Goal::PlanReviewOnly)
            .then(|| choice.plan.clone())
            .flatten();
        transfer.return_snapshot = None;
        transfer.renewed_assistant = false;
        task.dispatch_hold = true;
        task.queued = false;
        task.dispatch_plan = None;
        let scope = task
            .cowork
            .as_mut()
            .context("reader fence missing")?
            .completion
            .as_mut()
            .context("scope missing")?;
        scope.intent = if choice.goal == Goal::ImplementationShip {
            CompletionIntent::Ship
        } else {
            CompletionIntent::Verify
        };
        scope.contract_revision = choice.contract_revision.clone();
        scope.instruction = serde_json::to_string(&instruction)?;
        super::recovery::save(&ctx.store, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_continuation_prepared",
            json!({"transfer":journal.id,"choice":digest,"goal":choice.goal}),
        )?;
        Ok(())
    })?;
    finish_start(ctx, &task.id)
}

pub(super) fn finish_start(ctx: &Ctx, id: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    let journal = lifecycle::for_task(&ctx.store, &task)?.context("transfer missing")?;
    if !journal
        .transfer
        .as_ref()
        .is_some_and(|t| t.disposition == Disposition::Starting)
    {
        return Ok(task);
    }
    let selected = journal
        .transfer
        .as_ref()
        .context("transfer missing")?
        .code_pin
        .clone();
    ensure!(
        super::code_pin::current(ctx, &task)? == selected,
        "handoff_stale_choice: candidate/index/work changed; refresh the selected continuation"
    );
    let prepared = super::checkpoint(&ctx.store, &journal.id, false)?;
    super::adopt::prepare_task(ctx, id, &prepared)?;
    let adopted = ctx.store.get(id)?;
    let current_code = super::code_pin::current(ctx, &adopted)?;
    let seed = lifecycle::for_task(&ctx.store, &adopted)?
        .and_then(|j| j.transfer)
        .and_then(|t| t.seed_run);
    ensure!(
        match (&selected, &current_code) {
            (None, None) => true,
            (Some(expected), Some(actual)) =>
                expected.head == actual.head
                    && expected.state_sha256 == actual.state_sha256
                    && (expected.run == actual.run
                        || (expected.run.is_none() && actual.run == seed)),
            _ => false,
        },
        "handoff_stale_choice: source changed during adoption; refresh/reconcile"
    );
    let goal = journal.transfer.as_ref().context("transfer missing")?.goal;
    let submit = matches!(goal, Goal::ImplementationVerify | Goal::ImplementationShip)
        && adopted
            .current_run()
            .zip(current_code.as_ref())
            .is_some_and(|(run, code)| {
                code.run.as_deref() == Some(run.id.as_str())
                    && run.outcome == Some(corral_core::RunOutcome::Done)
                    && (run.gate.is_none()
                        || run.autopilot.as_ref().and_then(|s| s.sha.as_deref())
                            != Some(code.head.as_str()))
            });
    if submit && let Some(run) = adopted.current_run() {
        ensure!(
            super::git::git(
                &run.worktree,
                &["status", "--porcelain", "--untracked-files=all"],
                None
            )?
            .is_empty(),
            "handoff_candidate_dirty: preserve/take back the latest WIP before submitting a candidate"
        );
    }
    ctx.store.update(id, |task| {
        let mut journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        ensure!(
            transfer.disposition == Disposition::Starting
                && transfer.adoption_proven
                && transfer.contract_revision == task.contract_revision()?,
            "handoff_reconciling: source/spec changed during continuation adoption"
        );
        transfer.disposition = Disposition::Continuing;
        task.dispatch_hold = false;
        if submit
            && matches!(
                transfer.goal,
                Goal::ImplementationVerify | Goal::ImplementationShip
            )
        {
            crate::repair_cycle::submitted(&ctx.store, task)?;
            if let Some(run) = task.current_run_mut() {
                run.autopilot = Some(corral_core::AutopilotState {
                    stage: corral_core::AutoStage::Gating,
                    reason: Some("Explicit current-candidate continuation queued".into()),
                    sha: current_code.as_ref().map(|c| c.head.clone()),
                    at: corral_core::clock::now(),
                });
                task.status = Status::Review;
            }
        }
        if transfer.goal == Goal::PlanReviewOnly {
            task.status = Status::Review;
        } else if transfer.goal == Goal::PlanContinue && task.plans.is_empty() {
            task.status = Status::Ready;
        } else if task.runs.is_empty() {
            task.status = if task.plans.last().is_some_and(|p| p.approval.is_none()) {
                Status::Review
            } else {
                Status::Ready
            };
        }
        let goal = transfer.goal;
        journal.waiting = None;
        super::recovery::save(&ctx.store, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_continued",
            json!({"transfer":journal.id,"goal":goal}),
        )?;
        Ok(())
    })
}

/// Carry only the ordinary self-plan backlink. No task body, criteria, write scope or
/// implementation members change; every real scope change still needs a fresh instruction.
pub(crate) fn carry_plan_link(
    store: &corral_core::Store,
    task: &mut corral_core::TaskHeadMut,
    before: &str,
) -> Result<()> {
    use corral_core::TaskMut as _;
    let Some(mut journal) = lifecycle::for_task(store, &*task)? else {
        return Ok(());
    };
    let after = task.contract_revision()?;
    if before == after {
        return Ok(());
    }
    let plan = task
        .last_plan_head()?
        .context("plan link missing its source")?;
    ensure!(
        plan.approval.is_some() && plan.document.dependencies.is_empty(),
        "handoff_scope_changed: plan publication cannot widen transfer scope"
    );
    let transfer = journal.transfer.as_mut().context("transfer missing")?;
    ensure!(
        !lifecycle::plan_only(transfer.goal) && transfer.contract_revision == before,
        "handoff_scope_changed: self-plan source changed; renew the current-revision instruction"
    );
    transfer.contract_revision = after.clone();
    transfer.advance()?;
    task.edit_head_fields(|fields| {
        fields
            .cowork
            .as_mut()
            .context("reader fence missing")?
            .completion
            .as_mut()
            .context("scope missing")?
            .contract_revision = after.clone();
        Ok(())
    })?;
    super::recovery::save(store, &journal)?;
    store.log_event(
        Some(&task.id),
        "handoff_plan_link_carried",
        json!({"transfer":journal.id,"before":before,"after":after}),
    )?;
    Ok(())
}

pub(crate) fn save(ctx: &Ctx, id: &str, contract: &str, instruction: Instruction) -> Result<Task> {
    instruction.require_transfer()?;
    let _pipeline = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("Waiting for current landing/controller operation")?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("Waiting for current launch operation")?;
    let _gate = ctx
        .store
        .try_lock(&format!("gate-{id}"))?
        .context("Waiting for current gate admission")?;
    let saved = ctx.store.update(id, |task| {
        if task.status == Status::Done {
            return Ok(());
        } // Already landed; never claim rollback.
        ensure!(
            task.contract_revision()? == contract,
            "handoff_stale_choice: task/spec changed; refresh Save"
        );
        let mut journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
        ensure!(
            lifecycle::board_owner(&ctx.store, &journal)?,
            "handoff_owner: board is not current driver"
        );
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        ensure!(
            !matches!(
                transfer.disposition,
                Disposition::TakingBack | Disposition::Returned
            ),
            "handoff: take-back must settle first"
        );
        if !matches!(
            transfer.disposition,
            Disposition::Saved | Disposition::Saving
        ) || transfer.contract_revision != contract
        {
            transfer.record(instruction)?;
            transfer.advance()?;
        }
        transfer.contract_revision = contract.into();
        transfer.disposition = Disposition::Saving;
        task.dispatch_hold = true;
        task.queued = false;
        task.dispatch_plan = None;
        let scope = task
            .cowork
            .as_mut()
            .context("reader fence missing")?
            .completion
            .as_mut()
            .context("scope missing")?;
        scope.intent = CompletionIntent::Verify;
        scope.contract_revision = contract.into();
        super::recovery::save(&ctx.store, &journal)?;
        ctx.store.log_event(
            Some(&task.id),
            "handoff_save_held",
            json!({"transfer":journal.id}),
        )?;
        Ok(())
    })?;
    if saved.status == Status::Done {
        return Ok(saved);
    }
    finish_save(ctx, id)
}

/// Called with the controller, launch and gate leases, including by restart recovery.
/// Saving is a durable held intermediate state, not a claim that a writer has exited.
pub(super) fn finish_save(ctx: &Ctx, id: &str) -> Result<Task> {
    let holders = super::settlement::settle(ctx, id)?;
    ctx.store.update(id, |task| {
        let mut journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        ensure!(
            transfer.disposition == Disposition::Saving && task.dispatch_hold,
            "handoff_save_changed: pending save changed; reconcile"
        );
        ensure!(
            transfer.contract_revision == task.contract_revision()?,
            "handoff_scope_changed: task changed during stop; refresh the instruction"
        );
        if !holders.is_empty() {
            journal.waiting = Some(holders.join("; "));
            super::recovery::save(&ctx.store, &journal)?;
            return Ok(());
        }
        transfer.disposition = Disposition::Saved;
        journal.waiting = None;
        super::recovery::save(&ctx.store, &journal)?;
        ctx.store
            .log_event(Some(id), "handoff_saved", json!({"transfer":journal.id}))?;
        Ok(())
    })?;
    ensure!(holders.is_empty(), "handoff_saving: {}", holders.join("; "));
    ctx.store.get(id)
}
