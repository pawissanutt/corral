//! Owner-authenticated service entry. Native choice UI must authenticate an actual
//! selection before invoking these services; an input ID or model assertion is not consent.
use super::{
    lifecycle::{self, Disposition, Instruction},
    transfer::Choice,
};
use crate::{
    Ctx,
    assistant::owner_action::{self, OwnerAuthority},
};
use anyhow::{Context, Result, ensure};
use corral_core::{Task, TaskRef};
use serde_json::{Value, json};

fn instruction(ctx: &Ctx, session: &str, text: &str) -> Result<Instruction> {
    match owner_action::authority() {
        OwnerAuthority::ViaAssistant(via) => {
            ensure!(
                via.confirmed && via.session == session,
                "handoff_instruction: requires this assistant's confirmed scoped owner action"
            );
            let container = ctx.store.get_assistant_container(session)?;
            let input = container
                .owner_input(&via.input)
                .context("handoff_instruction: source owner input missing")?;
            ensure!(
                !input.truncated
                    && input.generation == via.generation
                    && input.sha256 == via.sha256,
                "handoff_instruction: provenance changed or incomplete"
            );
            Ok(Instruction::Assistant {
                input: input.clone(),
                request: via.request.clone(),
            })
        }
        OwnerAuthority::Shell => {
            ensure!(
                !crate::automation_controls::agent_entry()
                    && std::env::var("CORRAL_PM").as_deref() != Ok("1"),
                "handoff_instruction: an agent/PM cannot select a handoff goal for the owner"
            );
            Ok(Instruction::Shell {
                text: text.into(),
                at: corral_core::clock::now(),
            })
        }
    }
}

pub fn commit(ctx: &Ctx, choice: &Choice, selected_digest: &str) -> Result<Task> {
    let (_, package, _) = super::read_package(&ctx.store, &choice.package)?;
    let instruction = instruction(
        ctx,
        &package.assistant,
        &format!("handoff {} {selected_digest}", choice.package),
    )?;
    super::transfer::commit(ctx, choice, selected_digest, instruction)
}

pub fn take_back(
    ctx: &Ctx,
    task: &str,
    contract_revision: &str,
) -> Result<super::takeback::Returned> {
    let current = ctx.store.get(task)?;
    let journal =
        lifecycle::for_task(&ctx.store, &current)?.context("take_back: task has no transfer")?;
    let _instruction = instruction(
        ctx,
        &journal.assistant,
        &format!("take back {task} at {contract_revision}"),
    )?;
    super::takeback::apply(ctx, task, contract_revision)
}

pub fn read(ctx: &Ctx, id: &str) -> Result<Value> {
    let journal =
        super::read_journal(&ctx.store, id)?.context("handoff: preview is not yet reserved")?;
    let Some(transfer) = &journal.transfer else {
        return Ok(json!({"transfer":journal.id,"state":"preparing","owner":"local_fenced"}));
    };
    let task = ctx.store.get(&transfer.task)?;
    let waiting = match transfer.disposition {
        Disposition::Saved => Some("Saved for later; explicit continuation required".to_string()),
        Disposition::TakingBack => Some("Waiting for current operation to settle".to_string()),
        Disposition::Returned => {
            Some("Assistant prepare-only; separate Check/Ship required".to_string())
        }
        Disposition::Continuing if !transfer.ownership_committed || !transfer.adoption_proven => {
            Some("Reconciling held publication/adoption".to_string())
        }
        Disposition::Continuing if task.runs.is_empty() && task.plans.is_empty() => {
            crate::queue::capacity_reason(ctx, &ctx.store, &task, false)?
        }
        Disposition::Continuing => None,
    };
    let container = ctx.store.get_assistant_container(&journal.assistant)?;
    Ok(
        json!({"transfer":journal.id,"task":task.id,"contract_revision":task.contract_revision()?,
        "owner":container.workflow.as_ref().map(|w| &w.current.owner),"goal":transfer.goal,
        "disposition":transfer.disposition,"adoption_proven":transfer.adoption_proven,
        "source_retained":journal.source_retained,"waiting":waiting,
        "run":task.current_run().map(|r| &r.id),"plan":task.plans.last().map(|p| &p.id)}),
    )
}
