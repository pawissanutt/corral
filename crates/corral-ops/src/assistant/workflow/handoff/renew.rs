//! A returned implementation is prepare-only. Only a new confirmed current-driver
//! candidate submission can renew Verify/Ship; the original transfer instruction is history.
use super::{
    Goal, JOURNALS,
    lifecycle::{self, Disposition},
};
use anyhow::{Context, Result, ensure};
use corral_core::{
    CompletionDriver, CompletionIntent, CoworkReview, Task, TaskRef, assistant_workflow::ScopeOwner,
};

/// Refuse a new/changed completion intent BEFORE C1 rebases or publishes a candidate.
pub(crate) fn preflight(
    ctx: &crate::Ctx,
    task: &Task,
    intent: Option<CompletionIntent>,
) -> Result<()> {
    let Some(journal) = lifecycle::for_task(&ctx.store, task)? else {
        return Ok(());
    };
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if transfer.disposition == Disposition::Returned {
        let intent = intent
            .or_else(|| task.cowork.as_ref()?.completion.as_ref().map(|s| s.intent))
            .unwrap_or(CompletionIntent::Ship);
        let goal = if intent == CompletionIntent::Ship {
            Goal::ImplementationShip
        } else {
            Goal::ImplementationVerify
        };
        if !transfer.renewed_assistant || transfer.goal != goal {
            super::owner::require_completion_owner(ctx)?;
        }
    }
    Ok(())
}

pub(crate) fn submitted(
    ctx: &crate::Ctx,
    task: &mut Task,
    session: &str,
    scratch: &str,
) -> Result<()> {
    let Some(mut journal) = lifecycle::for_task(&ctx.store, task)? else {
        return Ok(());
    };
    let transfer = journal.transfer.as_mut().context("transfer missing")?;
    ensure!(
        transfer.disposition == Disposition::Returned && !lifecycle::plan_only(transfer.goal),
        "handoff_owner: take back implementation before submitting it here"
    );
    let container = ctx.store.get_assistant_container(&journal.assistant)?;
    let workflow = super::super::current(&container)?;
    ensure!(
        session == journal.assistant
            && transfer.returned_scratch.as_deref() == Some(scratch)
            && workflow.owner == ScopeOwner::Local,
        "handoff_owner: historical source is not the current author"
    );
    let contract = task.contract_revision()?;
    let cowork = task.cowork.as_mut().context("co-work missing")?;
    ensure!(
        cowork.owner_confirmed,
        "handoff_instruction: current Verify/Ship instruction must be confirmed"
    );
    let scope = cowork
        .completion
        .as_mut()
        .context("completion scope missing")?;
    ensure!(
        scope.driver
            == (CompletionDriver::Assistant {
                session: session.into(),
                scratch: scratch.into()
            })
            && scope.contract_revision == contract,
        "handoff_instruction: candidate instruction is stale"
    );
    let goal = if scope.intent == CompletionIntent::Ship {
        Goal::ImplementationShip
    } else {
        Goal::ImplementationVerify
    };
    let instruction = if !transfer.renewed_assistant || transfer.goal != goal {
        Some(super::owner::completion_instruction(
            ctx,
            &scope.instruction,
        )?)
    } else {
        // Bounded same-goal repair carries the original directive, even after the
        // source input/receipt cache expires. Never replace it with a default finish.
        scope.instruction = transfer
            .history
            .iter()
            .rev()
            .find_map(|t| match &t.instruction {
                lifecycle::Instruction::Completion { instruction, .. } => Some(instruction.clone()),
                _ => None,
            })
            .context(
                "completion_instruction_missing: recover the accepted current-scope directive",
            )?;
        None
    };
    cowork.review = CoworkReview::TransferV1;
    transfer.goal = goal;
    transfer.contract_revision = contract;
    if let Some(instruction) = instruction {
        transfer.record(instruction)?;
        transfer.advance()?;
    }
    transfer.renewed_assistant = true;
    task.dispatch_hold = false;
    ctx.store.save_record(JOURNALS, &journal.id, &journal)?;
    Ok(())
}
