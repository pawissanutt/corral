//! Woken parked runs re-acquire a coding slot before resuming; red-main fixes take capacity first.
use anyhow::Result;
use corral_core::{ReviewWork, SlotWait, Status, Store, Task, red_main};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::{Change, Ctx, QueueEntry, SlotUsage};

/// A run that released its slot, as `queue list` reports it.
#[derive(Debug, Clone, Serialize)]
pub struct ParkedRun {
    pub run: String,
    /// The tasks the run is (or was, once woken) waiting on.
    pub on: Vec<String>,
    /// Set once the wait fired and the run queued to re-acquire a slot.
    pub woken_since: Option<Timestamp>,
    /// The slot the run gave back.
    pub released: String,
}

fn woken(task: &Task) -> Option<&SlotWait> {
    task.current_run()
        .filter(|r| task.status.holds_slot() && r.outcome.is_none())
        .and_then(|r| r.obligations.slot_wait.as_ref())
}

fn parked(task: &Task) -> Option<ParkedRun> {
    let run = task.current_run().filter(|_| task.status.holds_slot())?;
    let slot = woken(task);
    let on = slot.map_or_else(|| run.parked_on(&task.id), |s| s.on.clone());
    (!on.is_empty()).then(|| ParkedRun {
        run: run.id.clone(),
        on,
        woken_since: slot.map(|s| s.since),
        released: format!(
            "{} coding slot{}",
            run.id,
            task.project
                .as_deref()
                .map(|p| format!(" (project {p})"))
                .unwrap_or_default()
        ),
    })
}

/// Precedence key: red-main fixes first, then woken runs oldest first; other work has none.
/// A claimant that cannot take a slot now (its project is saturated) reserves nothing.
fn key(store: &Store, task: &Task) -> Result<Option<(u8, Timestamp)>> {
    let key = claim(store, task)?;
    if key.is_some() && crate::slots::capacity_reason(store, task, 0)?.is_some() {
        return Ok(None);
    }
    Ok(key)
}

fn claim(store: &Store, task: &Task) -> Result<Option<(u8, Timestamp)>> {
    if let Some(slot) = woken(task) {
        return Ok(Some((u8::from(!red_main(task)), slot.since)));
    }
    let waiting = red_main(task)
        && task.status == Status::Ready
        && task.park.is_none()
        && !task.tracker
        && !task.dispatch_hold
        && store.config.ready_project_allowed(task.project.as_deref())
        && crate::queue::dependency_check(store, task)?.is_none();
    Ok(waiting.then_some((0, task.created)))
}

/// Tasks that take the next coding slot before `task`.
fn ahead_of(store: &Store, tasks: &[Task], task: &Task) -> Result<Vec<String>> {
    let mine = key(store, task)?;
    let mut ahead = Vec::new();
    for other in tasks.iter().filter(|o| o.id != task.id) {
        if let Some(theirs) = key(store, other)?
            && mine.is_none_or(|mine| (theirs, &other.id) < (mine, &task.id))
        {
            ahead.push(other.id.clone());
        }
    }
    Ok(ahead)
}

/// Refuses a coding slot that a red-main fix or an earlier woken run should take first.
pub(crate) fn precedence_reason(
    store: &Store,
    task: &Task,
    credit: usize,
) -> Result<Option<String>> {
    let tasks = store.list_shared()?;
    let ahead = ahead_of(store, &tasks, task)?;
    let limits = &store.config.limits;
    let used = SlotUsage::from_tasks(&tasks, limits)
        .total()
        .saturating_sub(credit);
    Ok(
        (!ahead.is_empty() && used + ahead.len() >= limits.running).then(|| {
            format!(
                "waiting: {} take the next coding slot first (red-main fixes, then woken runs)",
                ahead.join(", ")
            )
        }),
    )
}

/// Refuses a gate slot while a red-main fix waits for gate admission.
pub(crate) fn gate_precedence(store: &Store, task: &Task, active: usize) -> Result<Option<String>> {
    if red_main(task) {
        return Ok(None);
    }
    let waiting: Vec<_> = store
        .list_shared()?
        .iter()
        .filter(|o| o.id != task.id && red_main(o) && o.review_wait == Some(ReviewWork::Gate))
        .map(|o| o.id.clone())
        .collect();
    Ok(
        (!waiting.is_empty() && active + waiting.len() >= store.config.limits.review).then(|| {
            format!(
                "waiting: red-main fix {} takes the next gate slot",
                waiting.join(", ")
            )
        }),
    )
}

/// Grants a woken run a coding slot when one is free; true once it holds a slot again.
pub(crate) fn reacquire(ctx: &Ctx, id: &str) -> Result<bool> {
    let free = |store: &Store, task: &Task| -> Result<bool> {
        Ok(woken(task).is_some()
            && crate::slots::capacity_reason(store, task, 0)?.is_none()
            && precedence_reason(store, task, 0)?.is_none())
    };
    if !free(&ctx.store, &ctx.store.get(id)?)? {
        return Ok(false);
    }
    let (task, granted) = ctx.store.dispatch_update(id, |store, task| {
        if !free(store, task)? {
            return Ok(None);
        }
        let run = task.current_run_mut().filter(|r| r.outcome.is_none());
        Ok(run.and_then(|r| r.obligations.slot_wait.take().map(|s| (r.id.clone(), s))))
    })?;
    let Some((run, slot)) = granted else {
        return Ok(false);
    };
    crate::obligations::transition(
        ctx,
        &task.id,
        "slot_reacquired",
        json!({"run": run, "on": slot.on, "woken_since": slot.since}),
    )?;
    Ok(true)
}

/// Sync pass: woken runs re-acquire free slots in precedence order, before queued dispatch.
/// Candidates come from the pass's task snapshot; each grant re-reads its task under the board
/// lock, and runs woken later in the pass already tried to re-acquire where they woke.
pub(crate) fn reacquire_woken(ctx: &Ctx, tasks: &[Task]) -> Result<Vec<Change>> {
    let mut woken: Vec<_> = tasks
        .iter()
        .filter_map(|t| {
            let key = woken(t).map(|s| (u8::from(!red_main(t)), s.since))?;
            Some((key, t))
        })
        .collect();
    woken.sort_by(|a, b| (a.0, &a.1.id).cmp(&(b.0, &b.1.id)));
    let mut changes = Vec::new();
    for (_, task) in woken {
        if reacquire(ctx, &task.id)? {
            changes.push(Change {
                task: task.id.clone(),
                from: task.status,
                to: task.status,
                note: "woken parked run re-acquired a coding slot".into(),
                stage: None,
            });
        }
    }
    Ok(changes)
}

/// Queue rows for runs holding no slot: woken runs lead the queue, parked runs trail it.
pub(crate) fn queue_entries(tasks: &[Task]) -> (Vec<QueueEntry>, Vec<QueueEntry>) {
    let mut entries: Vec<_> = tasks
        .iter()
        .filter_map(|t| parked(t).map(|p| (t, p)))
        .collect();
    entries.sort_by_key(|(t, p)| (p.woken_since.is_none(), !red_main(t), p.woken_since));
    let (woken, parked): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .map(|(t, p)| QueueEntry {
            id: t.id.clone(),
            project: t.project.clone(),
            priority: t.priority.clone(),
            queued: false,
            reason: match p.woken_since {
                Some(_) => format!(
                    "woken from {}; queued to re-acquire a coding slot before {} resumes",
                    p.on.join(", "),
                    p.run
                ),
                None => format!("parked on {}; released the {}", p.on.join(", "), p.released),
            },
            plan: None,
            parked: Some(p),
        })
        .partition(|e| e.parked.as_ref().is_some_and(|p| p.woken_since.is_some()));
    (woken, parked)
}
