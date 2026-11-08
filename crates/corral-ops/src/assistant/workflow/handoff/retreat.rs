//! Cancel an unpublished selection, not its history. The same journal first fences
//! roll-forward, then returns only verified quiet local work in prepare-only mode.
use super::{
    Expected, Journal,
    lifecycle::{self, Disposition, Instruction},
    transfer::Target,
};
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::{
    CompletionDriver, CompletionIntent, Task, TaskRef,
    assistant_workflow::{ScopeOwner, WorkflowPhase},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub package: String,
    pub source: Expected,
    pub selection: Option<String>,
    pub card: Option<Target>,
    pub published_card: bool,
    pub scratch: Option<String>,
    pub source_link: Option<String>,
}
impl Choice {
    pub fn digest(&self) -> Result<String> {
        Ok(crate::verification::digest(&serde_json::to_vec(self)?))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub choice: Choice,
    pub digest: String,
    pub instruction: Instruction,
    pub complete: bool,
}
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Outcome {
    Waiting {
        package: String,
        holders: Vec<String>,
    },
    Retreated {
        package: String,
        held_card: Option<String>,
    },
}
fn source(ctx: &Ctx, journal: &Journal) -> Result<Expected> {
    let c = ctx.store.get_assistant_container(&journal.assistant)?;
    c.validate_protocol()?;
    let w = super::super::current(&c)?;
    ensure!(
        journal.reserved
            && w.id == journal.expected.workflow
            && w.owner == ScopeOwner::Local
            && matches!(
                w.phase,
                WorkflowPhase::HandoffPreparing | WorkflowPhase::HandoffReconciling
            )
            && w.handoff
                .as_ref()
                .is_some_and(|p| p.blob == journal.package.blob)
            && journal
                .transfer
                .as_ref()
                .is_none_or(|t| !t.ownership_committed),
        "handoff_retreat: ownership already published or source changed; use Take back for owned work"
    );
    Ok(Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    })
}

pub fn preview(ctx: &Ctx, package: &str) -> Result<Choice> {
    let journal = super::read_journal(&ctx.store, package)?
        .context("retreat requires a reserved selection")?;
    if let Some(retreat) = &journal.retreat {
        return Ok(retreat.choice.clone());
    }
    ensure!(
        !journal.cancelled,
        "handoff_consumed: selection was cancelled"
    );
    ensure!(
        journal.transfer.is_some() || journal.previous_handoff_known,
        "handoff_retreat: prior source lineage is unknown; retain/reconcile before cancelling"
    );
    let expected = source(ctx, &journal)?;
    let prepared = super::checkpoint(&ctx.store, package, false)?;
    let p = prepared.package();
    let assistant = ctx.store.get_assistant(&journal.assistant)?;
    let scratch = p
        .source_path
        .as_ref()
        .map(|path| {
            assistant
                .scratch
                .iter()
                .find(|s| &s.path == path && s.project == p.project)
                .map(|s| s.slug.clone())
                .context("handoff_retreat: retained source is not a registered scratch")
        })
        .transpose()?;
    let mut candidate = journal
        .transfer
        .as_ref()
        .map(|t| t.task.clone())
        .or_else(|| {
            journal
                .selection
                .as_ref()
                .and_then(|s| s.choice.into.as_ref().map(|t| t.task.clone()))
        });
    if candidate.is_none() {
        candidate = super::source::task(ctx, &prepared)?;
    }
    let mut card = None;
    if let Some(id) = candidate {
        let task = ctx.store.get(&id)?;
        let related = task
            .cowork
            .as_ref()
            .and_then(|c| c.assistant_driver())
            .is_some_and(|(session, slug)| {
                session == journal.assistant && scratch.as_deref().unwrap_or("") == slug
            });
        if journal.transfer.is_some() || related {
            card = Some(Target {
                task: task.id.clone(),
                contract_revision: task.contract_revision()?,
                run: task.current_run().map(|r| r.id.clone()),
                state: Some(super::target::pin(ctx, &task)?),
            });
        } // a never-published unrelated destination is left entirely untouched
    }
    Ok(Choice {
        package: package.into(),
        source: expected,
        selection: journal.selection.as_ref().map(|s| s.digest.clone()),
        card,
        published_card: journal.transfer.is_some(),
        scratch,
        source_link: assistant.linked_scratch.clone(),
    })
}
fn hold(task: &mut Task) -> Result<()> {
    task.dispatch_hold = true;
    task.queued = false;
    task.dispatch_plan = None;
    if let Some(c) = task.cowork.as_mut() {
        c.owner_confirmed = false;
        if let Some(scope) = c.completion.as_mut() {
            scope.intent = CompletionIntent::Verify;
        }
    }
    Ok(())
}
fn select(
    ctx: &Ctx,
    choice: &Choice,
    digest: &str,
    instruction: &Instruction,
    task: Option<&mut Task>,
) -> Result<()> {
    let mut journal =
        super::read_journal(&ctx.store, &choice.package)?.context("transfer missing")?;
    ensure!(
        journal.retreat.is_none() && !journal.cancelled && source(ctx, &journal)? == choice.source,
        "handoff_retreat_stale: selected source/retreat changed; refresh"
    );
    ensure!(
        journal.transfer.is_some() == choice.published_card
            && journal.selection.as_ref().map(|s| &s.digest) == choice.selection.as_ref(),
        "handoff_retreat_stale: publication/selection changed"
    );
    if let Some(task) = task {
        super::target::require(
            ctx,
            task,
            choice.card.as_ref().context("held card missing")?,
        )?;
        hold(task)?;
        if let Some(transfer) = journal.transfer.as_mut() {
            ensure!(
                transfer.task == task.id && !transfer.ownership_committed,
                "handoff_retreat: card ownership changed"
            );
            transfer.record(instruction.clone())?;
            transfer.contract_revision = task.contract_revision()?;
            transfer.disposition = Disposition::TakingBack;
            transfer.advance()?;
        }
    }
    journal.cancelled = true; // old commit/selection is irrevocably consumed BEFORE stop/return
    journal.retreat = Some(Record {
        choice: choice.clone(),
        digest: digest.into(),
        instruction: instruction.clone(),
        complete: false,
    });
    super::recovery::save(&ctx.store, &journal)
}

pub(crate) fn run(
    ctx: &Ctx,
    choice: &Choice,
    digest: &str,
    instruction: Instruction,
) -> Result<Outcome> {
    instruction.require_transfer()?;
    ensure!(
        choice.digest()? == digest,
        "handoff_retreat_stale: selected payload changed"
    );
    let _publication = ctx
        .store
        .try_lock(&format!(
            "handoff-{}",
            crate::verification::digest(choice.package.as_bytes())
        ))?
        .context("Waiting for current source publication")?;
    let mut leases = Vec::new();
    if let Some(card) = &choice.card {
        for name in ["autopilot", "launch", "gate", "plan-pilot"] {
            leases.push(
                ctx.store
                    .try_lock(&format!("{name}-{}", card.task))?
                    .context("Waiting for current card operation")?,
            );
        }
    }
    let journal = super::read_journal(&ctx.store, &choice.package)?.context("transfer missing")?;
    if let Some(selected) = &journal.retreat {
        ensure!(
            selected.digest == digest && selected.choice == *choice,
            "handoff_retreat_stale: another retreat already selected"
        );
        if selected.complete {
            return Ok(done(choice));
        } // historical replay, no current-state mutation
    } else {
        ensure!(
            preview(ctx, &choice.package)? == *choice,
            "handoff_retreat_stale: source/card changed; refresh"
        );
        if let Some(card) = &choice.card {
            ctx.store.update(&card.task, |task| {
                select(ctx, choice, digest, &instruction, Some(task))
            })?;
        } else {
            ctx.store
                .records_transaction(|| select(ctx, choice, digest, &instruction, None))?;
        }
    }
    let result = finish(ctx, choice);
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => wait(ctx, choice, vec![format!("{error:#}")]),
    }
}
fn done(choice: &Choice) -> Outcome {
    Outcome::Retreated {
        package: choice.package.clone(),
        held_card: choice.card.as_ref().map(|c| c.task.clone()),
    }
}
fn wait(ctx: &Ctx, choice: &Choice, holders: Vec<String>) -> Result<Outcome> {
    ctx.store.records_transaction(|| {
        let mut j =
            super::read_journal(&ctx.store, &choice.package)?.context("retreat disappeared")?;
        j.waiting = Some(holders.join("; "));
        super::recovery::save(&ctx.store, &j)
    })?;
    Ok(Outcome::Waiting {
        package: choice.package.clone(),
        holders,
    })
}
fn finish(ctx: &Ctx, choice: &Choice) -> Result<Outcome> {
    let journal = super::read_journal(&ctx.store, &choice.package)?.context("transfer missing")?;
    ensure!(
        source(ctx, &journal)? == choice.source,
        "handoff_retreat_stale: source revision changed; remains held"
    );
    super::source_boundary::attest(ctx, &choice.package)?; // an actual drain, never just a cancel/pane closure
    if let Some(card) = &choice.card {
        let holders = if choice.published_card {
            super::settlement::settle(ctx, &card.task)?
        } else {
            legacy_holders(&ctx.store.get(&card.task)?)
        };
        if !holders.is_empty() {
            return wait(ctx, choice, holders);
        }
        if choice.published_card {
            super::planner_draft::capture(ctx, &card.task)?;
        }
    }
    let (_, package, _) = super::read_package(&ctx.store, &choice.package)?;
    // Keep no-follow identity through the final writer transaction, not just an
    // earlier path check. Missing/replaced source remains a recovery wait.
    let source_root = package.source_path.as_ref().map(|path|
        corral_core::fs_guard::DirHandle::open(path).context("retreat source missing/changed; archive retained, restore scoped source before return")
    ).transpose()?;
    if let Some(card) = &choice.card {
        ctx.store.update(&card.task, |task| {
            finalize(ctx, choice, Some(task), source_root.as_ref())
        })?;
    } else {
        let _writer = ctx
            .store
            .try_lock("dispatch")?
            .context("Waiting for source writer transaction")?;
        ctx.store
            .records_transaction(|| finalize(ctx, choice, None, source_root.as_ref()))?;
    }
    Ok(done(choice))
}
pub(super) fn legacy_holders(task: &Task) -> Vec<String> {
    let mut holders = super::takeback::holders(task);
    if task.current_run().is_some_and(|r| r.agent_host.is_some()) {
        holders.push("retreat requires positive settlement of the existing worker binding; terminal/fenced metadata alone is insufficient".into());
    }
    holders
}

fn finalize(
    ctx: &Ctx,
    choice: &Choice,
    mut task: Option<&mut Task>,
    source_root: Option<&corral_core::fs_guard::DirHandle>,
) -> Result<()> {
    if let Some(root) = source_root {
        root.revalidate()?;
    }
    let mut journal =
        super::read_journal(&ctx.store, &choice.package)?.context("transfer missing")?;
    ensure!(
        source(ctx, &journal)? == choice.source
            && journal.cancelled
            && journal
                .retreat
                .as_ref()
                .is_some_and(|r| !r.complete && r.choice == *choice),
        "handoff_retreat_stale: source/publication changed"
    );
    let c = ctx.store.get_assistant_container(&journal.assistant)?;
    journal
        .source_boundary
        .as_ref()
        .context("retreat source drain proof missing")?
        .require_current(&ctx.store, &c, &journal)?;
    if let Some(task) = task.as_ref() {
        let card = choice.card.as_ref().context("card missing")?;
        let holders = if choice.published_card {
            super::takeback::holders(task)
        } else {
            legacy_holders(task)
        };
        ensure!(
            task.id == card.task
                && task.contract_revision()? == card.contract_revision
                && task.current_run().map(|r| &r.id) == card.run.as_ref()
                && holders.is_empty(),
            "handoff_retreat_stale: held card/scope changed or writer remains"
        );
    }
    let (_, package, _) = super::read_package(&ctx.store, &choice.package)?;
    ensure!(
        choice.published_card || journal.previous_handoff_known,
        "handoff_retreat: prior source lineage is unknown; retain/reconcile rather than guessing an empty source"
    );
    ctx.store
        .stage_assistant_container(&journal.assistant, |c, ended| {
            ensure!(
                !ended,
                "handoff_retreat: source session ended; keep held until an eligible return exists"
            );
            let index = c.workflow.as_mut().context("workflow missing")?;
            index.authority_epoch = index
                .authority_epoch
                .checked_add(1)
                .context("epoch overflow")?;
            let w = &mut index.current;
            w.authority_epoch = index.authority_epoch;
            w.state_revision = w
                .state_revision
                .checked_add(1)
                .context("revision overflow")?;
            w.execution = None;
            w.phase = if lifecycle::plan_only(package.goal) || w.current_plan.is_some() {
                WorkflowPhase::Planning
            } else {
                WorkflowPhase::Ordinary
            };
            if !choice.published_card {
                w.handoff = journal.previous_handoff.clone();
            }
            Ok(())
        })?;
    ctx.store.stage_assistant_session(&journal.assistant, |s| {
        ensure!(
            s.linked_scratch == choice.source_link
                && match (&choice.scratch, &package.source_path) {
                    (Some(slug), Some(path)) => s.scratch.iter().any(|entry| entry.slug == *slug
                        && entry.path == *path
                        && entry.project == package.project),
                    (None, None) => true,
                    _ => false,
                },
            "handoff_retreat_stale: source link/registered scratch changed; retain the held scope"
        );
        s.linked_scratch = choice.scratch.clone();
        Ok(())
    })?;
    if let Some(task) = task.as_mut() {
        hold(task)?;
        if let Some(transfer) = journal.transfer.as_mut() {
            ensure!(
                !transfer.ownership_committed,
                "handoff_retreat: ownership already published"
            );
            transfer.disposition = Disposition::Returned;
            transfer.renewed_assistant = false;
            transfer.returned_scratch = choice.scratch.clone();
            transfer.advance()?;
            transfer.goal = if lifecycle::plan_only(transfer.goal) {
                super::Goal::PlanContinue
            } else {
                super::Goal::ImplementationPrepare
            };
            let contract = task.contract_revision()?;
            let scope = task
                .cowork
                .as_mut()
                .context("card fence missing")?
                .completion
                .as_mut()
                .context("completion missing")?;
            scope.driver = CompletionDriver::Assistant {
                session: journal.assistant.clone(),
                scratch: choice.scratch.clone().unwrap_or_default(),
            };
            scope.intent = CompletionIntent::Verify;
            scope.contract_revision = contract;
        }
    }
    journal
        .retreat
        .as_mut()
        .context("retreat missing")?
        .complete = true;
    journal.waiting = None;
    super::recovery::save(&ctx.store, &journal)?;
    ctx.store.log_event(choice.card.as_ref().map(|c|c.task.as_str()),"handoff_retreated",
        serde_json::json!({"package":choice.package,"kept_card":choice.card.as_ref().map(|c|&c.task),"prepare_only":true}))?;
    if let Some(root) = source_root {
        root.revalidate()?;
    }
    Ok(())
}
