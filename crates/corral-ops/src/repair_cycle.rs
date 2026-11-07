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
    #[serde(default)]
    pub completed: bool,
    pub driver: Option<CompletionDriver>,
}

pub(crate) fn load(store: &Store, id: &str) -> Result<Option<Cycle>> {
    store.record(AREA, id)
}

/// Bind a durable issue identity. Transfer retries reuse it; a submitted no-op repair
/// followed by the same finding is a NEW cycle, not an unlimited free coding retry.
pub(crate) fn bind(
    store: &Store,
    key: &RepairKey,
    rounds: u32,
) -> Result<(RepairKey, Option<Cycle>)> {
    let old = load(store, &key.task)?;
    let reusable = old.as_ref().filter(|c| {
        !c.completed
            && c.key.head == key.head
            && c.key.reason == key.reason
            && if c.issued {
                c.round == rounds
            } else {
                Some(c.round) == rounds.checked_add(1)
            }
    });
    let cycle = match reusable {
        Some(c) => c.key.cycle,
        None => Some(
            old.as_ref()
                .and_then(|c| c.key.cycle)
                .unwrap_or(0)
                .checked_add(1)
                .context("repair cycle identity exhausted")?,
        ),
    };
    Ok((
        RepairKey {
            cycle,
            ..key.clone()
        },
        reusable.filter(|c| c.issued).cloned(),
    ))
}

/// Only an authorized author candidate submission closes a cycle; transfer does not.
/// Called inside the done/finish publication transaction, not by handoff/adoption.
pub(crate) fn submitted(store: &Store, task: &impl TaskRef) -> Result<()> {
    if let Some(mut cycle) = load(store, task.id())?
        && cycle.issued
        && !cycle.completed
        && cycle.round == *task.rounds()
    {
        cycle.completed = true;
        store.save_record(AREA, task.id(), &cycle)?;
        store.log_event(
            Some(task.id()),
            "repair_cycle_submitted",
            serde_json::json!({
                "key":cycle.key.digest()?, "round":cycle.round,
                "run":task.current_run_head()?.map(|r| r.id.clone())
            }),
        )?;
    }
    Ok(())
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
        ensure!(
            !cycle.completed,
            "completed repair cycle needs a fresh issue identity"
        );
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
            completed: false,
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
        !cycle.completed && cycle.key.digest()? == key.digest()? && cycle.round == round,
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
        && !cycle.completed
        && cycle.round == *task.rounds()
        && cycle.driver != driver
        && run
            .autopilot
            .as_ref()
            .is_some_and(|s| s.stage == corral_core::AutoStage::Changes))
    .then_some(cycle))
}
