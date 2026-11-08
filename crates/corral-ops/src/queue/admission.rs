//! Ready admission by the sync tick on task heads.
use anyhow::{Context, Result, bail};
use corral_core::{Status, TaskHead, TaskRef as _};
use serde_json::json;

use super::{
    authorized_ready, check_snapshot_head, dependency_check, pending_plan_reason, resolve_plan,
};
use crate::{Ctx, DispatchOpts};

/// Ready admission by the sync tick, on the task's head: the task's projection and its shell
/// decide admission, never its history.
pub(crate) fn enqueue_ready(ctx: &Ctx, id: &str, opts: &DispatchOpts) -> Result<TaskHead> {
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    let head = ctx.store.head(id)?;
    crate::tracker::check_action(&head, "enqueue tracker")?;
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{}", head.id))?
        .context("dispatch is already in progress")?;
    let head = ctx.store.head(&head.id)?;
    let task = head.shell_task()?;
    if !opts.plan
        && let Some(reason) = pending_plan_reason(&ctx.store, &task)?
    {
        bail!(
            "cannot enqueue {}: {reason}; use --plan to explicitly replan",
            head.id
        );
    }
    if head.launch.is_some() {
        bail!(
            "{} has an unfinished launch; resume or dequeue it first",
            head.id
        );
    }
    let planning = crate::plan_dispatch::needed(ctx, &head, opts);
    let approved = if planning {
        None
    } else {
        crate::approved_plan(ctx, &head)?
    };
    let plan = resolve_plan(ctx, &head, opts)?;
    let (head, ()) = ctx.store.dispatch_update_head(&head.id, |store, current| {
        check_snapshot_head(ctx, store, &head, current.head())?;
        if !planning {
            crate::dispatch::check_plan(ctx, current.head(), &plan.route, approved.as_ref())?;
        }
        if !authorized_ready(store, current.head()) {
            bail!("{} is not authorized Ready work", current.head().id);
        }
        if !matches!(
            current.status,
            Status::Ready | Status::Backlog | Status::Idea
        ) {
            bail!(
                "{} is {}; cannot enqueue",
                current.head().id,
                current.status.label()
            );
        }
        dependency_check(store, &current.head().shell_task()?)?;
        current.dispatch_plan = Some(plan.clone());
        current.status = Status::Ready;
        current.queued = true;
        current.dispatch_hold = false;
        Ok(())
    })?;
    ctx.store.observe_condition(
        Some(&head.id),
        "queued",
        json!({"plan": plan, "reason": "enqueue only"}),
    )?;
    Ok(head)
}
