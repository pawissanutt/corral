use anyhow::{Context, Result, bail, ensure};
use corral_core::{Status, Task};
use serde_json::json;

use crate::Ctx;

/// The caller holds the delivery lock shared with coding replies.
pub(super) fn reply(ctx: &Ctx, id: &str, text: &str) -> Result<Task> {
    let _launch = ctx
        .store
        .try_lock(&format!("launch-{id}"))?
        .context("planner launch is in progress; retry reply")?;
    let task = ctx.store.get(id)?;
    let run = task.plan_runs.last().context("task has no planner run")?;
    ensure!(
        run.holds_slot(),
        "{id}: planner {} is {:?}; no live planner accepts replies; use `corral dispatch {id} --plan` to replan",
        run.id,
        run.state
    );
    ensure!(
        crate::session_scope::owns_task(ctx, &task)?,
        "{id}: selected herdr session does not own the planner pane"
    );
    ensure!(
        run.spec_revision == crate::plan_spec_revision(&task)?,
        "{id}: planner {} has a stale task spec; explicitly replan",
        run.id
    );
    let reference = run
        .herdr
        .as_ref()
        .with_context(|| format!("{id}: planner {} has no pane attached", run.id))?;
    let pane = &reference.pane_id;
    let info = ctx
        .herdr
        .pane_get(pane)
        .with_context(|| format!("{id}: inspecting planner pane {pane}"))?
        .with_context(|| format!("{id}: planner pane {pane} is gone; explicitly replan"))?;
    ensure!(
        info.workspace_id == reference.workspace_id,
        "{id}: planner pane {pane} belongs to a different workspace"
    );
    ensure!(
        crate::plan_lifecycle::exit_reason(run)?.is_none()
            && !crate::plan_lifecycle::shell_returned(ctx, run)?,
        "{id}: planner {} exited or returned to its shell; explicitly replan",
        run.id
    );
    ctx.store.dispatch_update(id, |store, current| {
        crate::queue::check_snapshot(ctx, store, &task, current)?;
        ensure!(
            crate::session_scope::owns_tasks(ctx, store, std::slice::from_ref(current))?,
            "{id}: selected herdr session does not own the planner pane"
        );
        let run = current.plan_runs.last_mut().context("planner disappeared")?;
        if run.pending_reply.as_deref().is_some_and(|pending| pending != text) {
            bail!("{id}: planner reply pending; resend with `corral reply {id} --resend` before sending another");
        }
        run.pending_reply = Some(text.to_string());
        Ok(())
    })?;
    let delivery = ctx
        .herdr
        .agent_prompt(pane, text)
        .with_context(|| format!("{id}: delivering planner reply"))?;
    ctx.store.log_event(
        Some(id),
        "plan_replied",
        json!({
            "target":"plan", "run":run.id, "pane":pane, "text":text,
            "via":delivery.via, "submitted":delivery.submitted,
            "attempts":delivery.attempts, "error":delivery.error,
        }),
    )?;
    ensure!(
        delivery.submitted,
        "{id}: planner reply not delivered ({}); pending, retry with `corral reply {id} --resend`",
        delivery
            .error
            .as_deref()
            .unwrap_or("submission was not observed")
    );
    ctx.store.update(id, |current| {
        if let Some(current_run) = current.plan_runs.last_mut()
            && current_run.id == run.id
            && current_run.holds_slot()
            && current_run.pending_reply.as_deref() == Some(text)
        {
            current_run.pending_reply = None;
            if current.status == Status::NeedsMe {
                current.status = Status::Running;
            }
        }
        Ok(())
    })
}
