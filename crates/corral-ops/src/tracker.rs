//! Trackers aggregate child completion; they never launch agent work.
use anyhow::{Result, bail};
use corral_core::{Status, Task};
use serde_json::json;

use crate::{Change, Ctx};

pub(crate) fn check_action(task: &Task, action: &str) -> Result<()> {
    if task.tracker {
        bail!(
            "{} is a tracker; cannot {action}; dispatch its child tasks instead",
            task.id
        );
    }
    Ok(())
}

/// Read both the tracker and its children under the board writer lock. Returning
/// false from try_update preserves timestamps and concurrent owner edits.
pub(crate) fn reconcile_one(ctx: &Ctx, id: &str) -> Result<Option<Change>> {
    let mut from = None;
    let saved = ctx.store.try_update(id, |task| {
        if !task.tracker
            || matches!(task.status, Status::Done | Status::Dropped)
            || task.children.is_empty()
        {
            return Ok(false);
        }
        let children = ctx.store.list()?;
        if !task.children.iter().all(|id| {
            children
                .iter()
                .any(|child| child.id == *id && child.status == Status::Done)
        }) {
            return Ok(false);
        }
        from = Some(task.status);
        task.status = Status::Done;
        task.queued = false;
        task.dispatch_plan = None;
        Ok(true)
    })?;
    let Some(task) = saved else {
        return Ok(None);
    };
    ctx.store.log_event(
        Some(&task.id),
        "tracker_completed",
        json!({"from": from, "to": "done", "children": task.children}),
    )?;
    Ok(from.map(|from| Change {
        task: task.id,
        from,
        to: Status::Done,
        note: "all tracker children are Done".into(),
        stage: None,
    }))
}

pub(crate) fn reconcile(ctx: &Ctx) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for task in ctx.store.list()? {
        if task.tracker
            && let Some(change) = reconcile_one(ctx, &task.id)?
        {
            changes.push(change);
        }
    }
    Ok(changes)
}

#[cfg(test)]
#[path = "tracker_tests.rs"]
mod tests;
