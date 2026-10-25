//! The turn records of `corral agent cancel` (T-252 recovery, row 2 of the turn table). Cancel
//! marks the open turn it can see before it asks the host, takes the mark back when the host did
//! not cancel, and binds the cancellation to the turn the host names as cancelled. The host
//! reports its turns asynchronously, so that turn may not be recorded yet, or may have ended
//! unmarked already: an ended turn is reclassified from the record, and a turn not recorded yet
//! keeps a pending cancellation the sync hook applies once its reports arrive, before OD11 or a
//! provider retry reads the turn.
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::Task;
use corral_core::agent::{self, AgentTurn, ContainerRef, TurnClass};
use serde::{Deserialize, Serialize};

use crate::Ctx;

/// Turns (generation, seq) the host confirmed cancelled before their reports arrived.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Pending {
    turns: Vec<(u32, u64)>,
}

fn pending_path(ctx: &Ctx, container: &ContainerRef) -> PathBuf {
    let name = container.key().replace('/', "_");
    ctx.store
        .state_dir()
        .join("agent-cancel")
        .join(format!("{name}.json"))
}

fn load(ctx: &Ctx, container: &ContainerRef) -> Result<Pending> {
    let path = pending_path(ctx, container);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Pending::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save(ctx: &Ctx, container: &ContainerRef, pending: &Pending) -> Result<()> {
    let path = pending_path(ctx, container);
    if pending.turns.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("removing {}", path.display()))
            }
            _ => Ok(()),
        };
    }
    let dir = path.parent().context("cancel state path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let staged = path.with_extension("tmp");
    std::fs::write(&staged, serde_json::to_vec(pending)?)
        .with_context(|| format!("writing {}", staged.display()))?;
    std::fs::rename(&staged, &path).with_context(|| format!("replacing {}", path.display()))
}

/// Marks generation `n`'s open turn (or open turn `turn`) cancelled before the host is asked,
/// so a `turn_ended` that comes first classifies `cancelled`; returns the marked turn.
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

/// Binds the cancellation the host confirmed to its turn `seq`: an open turn is marked, an ended
/// one reclassified, and one not recorded yet waits as a pending cancellation.
pub(crate) fn confirm(ctx: &Ctx, container: &ContainerRef, n: u32, seq: u64) -> Result<()> {
    if apply(ctx, container, n, seq)? {
        return Ok(());
    }
    let mut pending = load(ctx, container)?;
    if !pending.turns.contains(&(n, seq)) {
        pending.turns.push((n, seq));
        save(ctx, container, &pending)?;
    }
    Ok(())
}

/// Records the cancellation on turn `seq` of generation `n`; false while it is not recorded.
fn apply(ctx: &Ctx, container: &ContainerRef, n: u32, seq: u64) -> Result<bool> {
    let found = edit(
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
    Ok(found.is_some())
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

fn containers(task: &Task) -> Vec<ContainerRef> {
    let mut out = Vec::new();
    for run in &task.runs {
        out.push(ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        });
        for child in &run.review_runs {
            out.push(ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            });
        }
    }
    out
}

/// Sync: applies each pending cancellation whose turn is recorded now. One whose container closed
/// or whose generation was fenced is dropped; the rest wait for their reports.
pub fn settle(ctx: &Ctx, task_id: &str) -> Result<()> {
    let task = ctx.store.get(task_id)?;
    for container in containers(&task) {
        if !pending_path(ctx, &container).exists() {
            continue;
        }
        let pending = load(ctx, &container)?;
        let closed = agent::container_closure(&task, &container).is_some();
        let state = agent::container_state(&task, &container);
        let mut waiting = Vec::new();
        for (n, seq) in pending.turns {
            let live = !closed
                && state.is_some_and(|s| s.generation(n).is_some_and(|g| g.fenced.is_none()));
            if live && !apply(ctx, &container, n, seq)? {
                waiting.push((n, seq));
            }
        }
        save(ctx, &container, &Pending { turns: waiting })?;
    }
    Ok(())
}
