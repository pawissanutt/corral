//! The turn records of `corral agent cancel` (T-252 recovery, row 2 of the turn table). The host
//! is the authority on what it cancelled: its own `turn_ended` report says so, and the event op
//! classifies that turn `cancelled` in the transaction that ends it, however the host's reports,
//! the cancel's reply and sync's consumers interleave. Cancel also marks the open turn it can see
//! before it asks the host, takes that mark back when the host did not cancel it, and binds the
//! host's answer to a turn already recorded, reclassifying one that ended before either fact.
use anyhow::{Context, Result};
use corral_core::agent::{self, AgentTurn, ContainerRef, TurnClass};

use crate::Ctx;

/// Marks generation `n`'s open turn (or open turn `turn`) cancelled before the host is asked;
/// returns the marked turn.
pub(crate) fn mark(
    ctx: &Ctx,
    container: &ContainerRef,
    n: u32,
    turn: Option<u64>,
) -> Result<Option<u64>> {
    let named = |t: &AgentTurn| {
        t.generation == n && turn.is_none_or(|seq| t.seq == seq) && t.started.is_some()
    };
    edit(
        ctx,
        container,
        |turns| turns.iter().rposition(|t| named(t) && t.ended.is_none()),
        |turn| {
            turn.class = Some(TurnClass::Cancelled);
            false
        },
    )
}

/// Takes back the mark on turn `seq` the host did not cancel, reclassifying it from the record
/// when it ended meanwhile.
pub(crate) fn unmark(ctx: &Ctx, container: &ContainerRef, n: u32, seq: u64) -> Result<()> {
    let marked =
        |t: &AgentTurn| t.generation == n && t.seq == seq && t.class == Some(TurnClass::Cancelled);
    edit(
        ctx,
        container,
        |turns| turns.iter().rposition(marked),
        |turn| {
            turn.class = None;
            turn.ended.is_some()
        },
    )?;
    Ok(())
}

/// Binds the cancellation the host confirmed to its turn `seq` when that turn is recorded: an
/// open one is marked and an ended one reclassified (`completed` still wins). A turn not recorded
/// yet needs nothing here, since its own `turn_ended` report carries the cancellation.
pub(crate) fn confirm(ctx: &Ctx, container: &ContainerRef, n: u32, seq: u64) -> Result<()> {
    edit(
        ctx,
        container,
        |turns| {
            turns
                .iter()
                .position(|t| t.generation == n && t.seq == seq && t.started.is_some())
        },
        |turn| {
            let changed = turn.class != Some(TurnClass::Cancelled);
            turn.class = Some(TurnClass::Cancelled);
            changed && turn.ended.is_some()
        },
    )?;
    Ok(())
}

/// In one transaction: finds a turn of the container, changes it, and reclassifies it from the
/// record when `change` says so. Returns the turn's seq when one was found.
fn edit(
    ctx: &Ctx,
    container: &ContainerRef,
    find: impl Fn(&[AgentTurn]) -> Option<usize>,
    change: impl Fn(&mut AgentTurn) -> bool,
) -> Result<Option<u64>> {
    let Some(task_id) = container.task() else {
        return Ok(None);
    };
    let mut found = None;
    ctx.store.try_update(task_id, |task| {
        found = None;
        let closure = agent::container_closure(task, container);
        let state =
            agent::container_state_mut(task, container).context("the agent binding disappeared")?;
        let Some(index) = find(&state.turns) else {
            return Ok(false);
        };
        found = Some(state.turns[index].seq);
        let before = state.turns[index].class;
        if change(&mut state.turns[index]) {
            super::super::turns::classify_ended(state, closure, index);
        }
        Ok(state.turns[index].class != before)
    })?;
    Ok(found)
}
