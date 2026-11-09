//! An admitted planner dispatch may close its previous writer before allocating D<n>.
//! This is an internal transition of the existing instruction/journal, not new consent.
use anyhow::{Context, Result, ensure};
use corral_core::{OperatingActionKind, Store, Task, TaskRef};
use serde::{Deserialize, Serialize};

use super::{
    Goal, Journal,
    lifecycle::{self, Disposition},
};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    contract: String,
    planner: Option<String>,
    worker: Option<String>,
    goal: Goal,
    choice: String,
}

impl Pending {
    fn of(task: &(impl TaskRef + ?Sized), transfer: &lifecycle::Transfer) -> Result<Self> {
        Ok(Self {
            contract: task.contract_revision()?,
            planner: task.last_plan_run_head()?.map(|r| r.id.clone()),
            worker: task.current_run_head()?.map(|r| r.id.clone()),
            goal: transfer.goal,
            choice: transfer.choice_digest.clone(),
        })
    }
}

/// A hold alone cannot authorize native stop. Require the exact durable dispatch
/// checkpoint and still-current owner/scope/previous attempts on every recheck.
pub(crate) fn require(
    store: &Store,
    task: &(impl TaskRef + ?Sized),
    journal: &Journal,
) -> Result<()> {
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    ensure!(
        *task.dispatch_hold() && transfer.disposition == Disposition::CheckpointingPlan,
        "handoff_plan_checkpoint: no held planner preparation"
    );
    ensure!(
        transfer.ownership_committed
            && transfer.adoption_proven
            && lifecycle::board_owner(store, journal)?,
        "handoff_plan_checkpoint: current driver changed"
    );
    ensure!(
        transfer.contract_revision == task.contract_revision()?
            && transfer.plan_checkpoint.as_ref() == Some(&Pending::of(task, transfer)?),
        "handoff_plan_checkpoint: scope, intent or previous attempt changed"
    );
    ensure!(
        !matches!(
            task.status(),
            corral_core::Status::Done | corral_core::Status::Dropped
        ),
        "handoff_plan_checkpoint: task ended"
    );
    Ok(())
}

pub(crate) fn before_plan(ctx: &Ctx, task: &impl TaskRef) -> Result<()> {
    if !task
        .cowork()
        .as_ref()
        .is_some_and(|c| c.review == corral_core::CoworkReview::TransferV1)
    {
        return Ok(());
    }
    // Dispatch already owns launch; these also exclude Save/return/landing.
    let _pipeline = ctx
        .store
        .try_lock(&format!("autopilot-{}", task.id()))?
        .context("Waiting for prior controller")?;
    let _gate = ctx
        .store
        .try_lock(&format!("gate-{}", task.id()))?
        .context("Waiting for prior gate")?;
    let selected = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
    let transfer = selected.transfer.as_ref().context("transfer missing")?;
    let expected = Pending::of(task, transfer)?;
    ctx.store.update(task.id(), |current| {
        let mut journal = lifecycle::for_task(&ctx.store, current)?.context("transfer missing")?;
        let transfer = journal.transfer.as_ref().context("transfer missing")?;
        ensure!(
            journal.id == selected.id
                && transfer.revision
                    == selected
                        .transfer
                        .as_ref()
                        .context("transfer missing")?
                        .revision
                && Pending::of(current, transfer)? == expected,
            "handoff_plan_changed: scope/run changed before checkpoint"
        );
        super::admission::require(&ctx.store, current, OperatingActionKind::Plan, false)?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        transfer.disposition = Disposition::CheckpointingPlan;
        transfer.plan_checkpoint = Some(expected.clone());
        transfer.advance()?;
        current.dispatch_hold = true;
        journal.waiting = Some("Settling prior planning context before the next dispatch".into());
        super::recovery::save(&ctx.store, &journal)
    })?;
    finish(ctx, task.id())
}

/// Caller holds pipeline/launch/gate leases. Recovery does not allocate a planner;
/// it restores only this admitted continuing instruction for ordinary scheduling.
pub(super) fn finish(ctx: &Ctx, id: &str) -> Result<()> {
    let selected =
        lifecycle::for_task(&ctx.store, &ctx.store.head(id)?)?.context("transfer missing")?;
    let result = finish_inner(ctx, id, &selected);
    if let Err(error) = &result {
        ctx.store.records_transaction(|| {
            let mut current =
                super::read_journal(&ctx.store, &selected.id)?.context("transfer missing")?;
            if current.transfer.as_ref().is_some_and(|t| {
                t.disposition == Disposition::CheckpointingPlan
                    && t.plan_checkpoint
                        == selected
                            .transfer
                            .as_ref()
                            .and_then(|s| s.plan_checkpoint.clone())
            }) {
                current.waiting = Some(format!("{error:#}"));
                super::recovery::save(&ctx.store, &current)?;
            }
            Ok(())
        })?;
    }
    result
}

fn finish_inner(ctx: &Ctx, id: &str, selected: &Journal) -> Result<()> {
    require(&ctx.store, &ctx.store.head(id)?, selected)?;
    let holders = super::settlement::settle(ctx, id)?;
    ensure!(
        holders.is_empty(),
        "handoff_draft_busy: {}",
        holders.join("; ")
    );
    super::planner_draft::capture(ctx, id)?;
    ctx.store.update(id, |current: &mut Task| {
        let mut journal = lifecycle::for_task(&ctx.store, current)?.context("transfer missing")?;
        ensure!(
            journal.id == selected.id,
            "handoff_plan_changed: transfer changed"
        );
        require(&ctx.store, current, &journal)?;
        super::planner_draft::require_current(ctx, current, &journal)?;
        let transfer = journal.transfer.as_mut().context("transfer missing")?;
        transfer.disposition = Disposition::Continuing;
        transfer.plan_checkpoint = None;
        transfer.advance()?;
        current.dispatch_hold = false;
        journal.waiting = None;
        // Stage both records before admission reads them; a refusal rolls back
        // the state/index and task hold together, never publishing a half-release.
        super::recovery::save(&ctx.store, &journal)?;
        super::admission::require(&ctx.store, current, OperatingActionKind::Plan, false)
    })?;
    Ok(())
}
