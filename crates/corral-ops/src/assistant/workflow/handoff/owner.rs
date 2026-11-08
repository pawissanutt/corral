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

pub fn require_entry(ctx: &Ctx) -> Result<()> {
    ensure!(
        matches!(owner_action::authority(), OwnerAuthority::Shell),
        "handoff_unavailable: assistant-origin entry needs an authenticated named-action/choice consumer"
    );
    crate::automation_controls::require_automation_owner(&ctx.store)
}

/// Bootstrap only a legacy ordinary source, never a planning-to-execution transition.
/// The input is provenance; the guarded owner command, not its ID, authorizes this entry.
pub fn initialize(ctx: &Ctx, session: &str, input: &str) -> Result<super::Expected> {
    require_entry(ctx)?;
    ensure!(
        super::super::boundary::activities(&ctx.store, session)?.is_empty(),
        "handoff_busy: source effects must settle before preparation"
    );
    ctx.store.settle_assistant_container(session, |c, _| {
        ensure!(
            c.workflow.is_none(),
            "handoff_source: existing workflow must be used without changing its mode"
        );
        super::super::prepare_entry(&ctx.store, session, c, input)?;
        c.workflow
            .as_mut()
            .context("workflow missing")?
            .current
            .phase = corral_core::assistant_workflow::WorkflowPhase::Ordinary;
        Ok(())
    })?;
    let c = ctx.store.get_assistant_container(session)?;
    let w = super::super::current(&c)?;
    Ok(super::Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: None,
    })
}

pub fn prepare(
    ctx: &Ctx,
    session: &str,
    request: super::Request,
    scratch: Option<&str>,
) -> Result<super::Prepared> {
    require_entry(ctx)?;
    let c = ctx.store.get_assistant_container(session)?;
    let w = super::super::current(&c)?;
    let expected = super::Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    };
    let assistant = ctx.store.get_assistant(session)?;
    let sources: Vec<_> = assistant
        .scratch
        .iter()
        .filter(|s| s.project == request.project)
        .collect();
    let source = match scratch {
        Some(slug) => Some(
            *sources
                .iter()
                .find(|s| s.slug == slug)
                .context("handoff_source: selected scratch not found in project")?,
        ),
        None => {
            ensure!(
                sources.len() <= 1,
                "handoff_source: choose one explicit source scratch"
            );
            sources.first().copied()
        }
    };
    super::prepare(
        &ctx.store,
        session,
        &c,
        &expected,
        request,
        source.map(|s| (s.path.as_path(), s.base.as_str(), s.branch.as_str())),
    )
}

pub(crate) fn instruction(ctx: &Ctx, _session: &str, text: &str) -> Result<Instruction> {
    // An ordinary finish confirmation is NOT transfer consent. Native assistant entry
    // stays closed until C4 installs its payload-bound stable-option consumer.
    ensure!(
        matches!(owner_action::authority(), OwnerAuthority::Shell),
        "handoff_unavailable: assistant-origin transfer needs an authenticated named-action/choice consumer; ordinary finish confirmation is insufficient"
    );
    crate::automation_controls::require_automation_owner(&ctx.store)?;
    Ok(Instruction::Shell {
        text: text.into(),
        at: corral_core::clock::now(),
    })
}

/// Only a provenance copy for the separately guarded C1 candidate submission. This
/// does not authorize a handoff, ownership change, dispatch or native plan execution.
pub(crate) fn require_completion_owner(ctx: &Ctx) -> Result<()> {
    // A return revoked the old loop. A generic finish confirmation cannot prove that
    // it was delivered for the new returned scope; C4 must consume that native choice.
    ensure!(
        matches!(owner_action::authority(), OwnerAuthority::Shell),
        "completion_unavailable: renewed returned-scope intent needs the guarded owner entry or an authenticated current-scope native consumer"
    );
    crate::automation_controls::require_automation_owner(&ctx.store)
}

pub(crate) fn completion_instruction(ctx: &Ctx, text: &str) -> Result<Instruction> {
    require_completion_owner(ctx)?;
    Ok(Instruction::Completion {
        instruction: text.into(),
        via: None,
        at: corral_core::clock::now(),
    })
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

pub fn start(
    ctx: &Ctx,
    choice: &super::continuation::Choice,
    selected_digest: &str,
) -> Result<Task> {
    let journal = super::read_journal(&ctx.store, &choice.transfer)?.context("transfer missing")?;
    let instruction = instruction(
        ctx,
        &journal.assistant,
        &format!("continue {} {selected_digest}", choice.task),
    )?;
    super::continuation::start(ctx, choice, selected_digest, instruction)
}

pub fn save(ctx: &Ctx, task: &str, contract_revision: &str) -> Result<Task> {
    let current = ctx.store.get(task)?;
    let journal = lifecycle::for_task(&ctx.store, &current)?.context("transfer missing")?;
    let instruction = instruction(
        ctx,
        &journal.assistant,
        &format!("save {task} at {contract_revision}"),
    )?;
    super::continuation::save(ctx, task, contract_revision, instruction)
}

pub fn take_back(
    ctx: &Ctx,
    task: &str,
    contract_revision: &str,
) -> Result<super::takeback::Returned> {
    let current = ctx.store.get(task)?;
    let journal =
        lifecycle::for_task(&ctx.store, &current)?.context("take_back: task has no transfer")?;
    let instruction = instruction(
        ctx,
        &journal.assistant,
        &format!("take back {task} at {contract_revision}"),
    )?;
    super::takeback::apply(ctx, task, contract_revision, instruction)
}

pub fn read(ctx: &Ctx, id: &str) -> Result<Value> {
    let journal =
        super::read_journal(&ctx.store, id)?.context("handoff: preview is not yet reserved")?;
    let Some(transfer) = &journal.transfer else {
        return Ok(
            json!({"transfer":journal.id,"state":"preparing","owner":"local_fenced", "waiting":journal.waiting, "source_drained":journal.source_boundary.is_some()}),
        );
    };
    let task = ctx.store.get(&transfer.task)?;
    let waiting = match transfer.disposition {
        Disposition::Saving => Some("Stopping scoped writers; board remains held".to_string()),
        Disposition::Saved => Some("Saved for later; explicit continuation required".to_string()),
        Disposition::Starting => {
            Some("Reconciling the selected current-revision continuation".to_string())
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
        Disposition::Continuing
            if matches!(
                task.status,
                corral_core::Status::Ready | corral_core::Status::Backlog
            ) =>
        {
            crate::queue::capacity_reason(ctx, &ctx.store, &task, false)?
                .or_else(|| Some("Queued for normal admission".into()))
        }
        Disposition::Continuing => None,
    };
    let container = ctx.store.get_assistant_container(&journal.assistant)?;
    Ok(
        json!({"transfer":journal.id,"task":task.id,"contract_revision":task.contract_revision()?,
        "owner":container.workflow.as_ref().map(|w| &w.current.owner),"goal":transfer.goal,
        "disposition":transfer.disposition,"adoption_proven":transfer.adoption_proven,
        "source_retained":journal.source_retained,"revision":transfer.revision,
        "waiting":journal.waiting.as_ref().or(waiting.as_ref()),
        "findings":task.plans.last().and_then(|p| p.reviews.last()).map(|r| json!({"review":r.id,"reviewer":r.reviewer,"verdict":r.verdict,"notes":r.notes})),
        "plan_only":lifecycle::plan_only(transfer.goal),
        "run":task.current_run().map(|r| &r.id),"plan":task.plans.last().map(|p| &p.id)}),
    )
}
