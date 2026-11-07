//! One issued repair cycle survives driver/run replacement. Preparation is not a charge.
use crate::{Ctx, integration::RepairKey};
use anyhow::{Context, Result, ensure};
use corral_core::{CompletionDriver, Store, TaskRef};
use serde::{Deserialize, Serialize};
const AREA: &str = "completion-repair-cycle";
#[cfg(test)]
#[path = "repair_cycle_tests.rs"]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Cycle {
    pub key: RepairKey,
    pub round: u32,
    pub summary: String,
    pub details: String,
    pub issued: bool,
    pub driver: Option<CompletionDriver>,
}

pub(crate) fn load(store: &Store, id: &str) -> Result<Option<Cycle>> {
    store.record(AREA, id)
}

pub(crate) fn prepare(
    ctx: &Ctx,
    key: &RepairKey,
    round: u32,
    summary: &str,
    details: &str,
) -> Result<()> {
    if let Some(cycle) = load(&ctx.store, &key.task)?
        && cycle.key.digest()? == key.digest()?
        && cycle.issued
    {
        ensure!(cycle.round == round, "repair cycle charge changed");
        return Ok(());
    }
    ctx.store.save_record(
        AREA,
        &key.task,
        &Cycle {
            key: key.clone(),
            round,
            summary: summary.into(),
            details: details.into(),
            issued: false,
            driver: key.driver.clone(),
        },
    )
}

/// Joins the executor's durable prepare transaction. Handoff changes delivery, never the round.
pub(crate) fn issue(store: &Store, task: &impl TaskRef, key: &RepairKey, round: u32) -> Result<()> {
    let Some(mut cycle) = load(store, task.id())? else {
        return Ok(());
    }; // older/manual executor
    ensure!(
        cycle.key.digest()? == key.digest()? && cycle.round == round,
        "repair cycle changed before issue"
    );
    cycle.issued = true;
    cycle.key = key.clone();
    cycle.driver = key.driver.clone();
    store.save_record(AREA, task.id(), &cycle)
}

pub(crate) fn pending(store: &Store, task: &impl TaskRef) -> Result<Option<Cycle>> {
    let Some(cycle) = load(store, task.id())? else {
        return Ok(None);
    };
    let run = task
        .current_run_head()?
        .context("repair cycle has no run")?;
    let driver = RepairKey::driver(task);
    Ok((cycle.issued
        && cycle.driver != driver
        && run
            .autopilot
            .as_ref()
            .is_some_and(|s| s.stage == corral_core::AutoStage::Changes))
    .then_some(cycle))
}
