//! Rows 21-23 of a sync pass: each open head's obligations, each owned open head's
//! reconciliation with its panes, then native heads that still owe work.
use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use corral_core::{Board, TaskHead};
use corral_herdr::PaneInfo;

use super::{
    Change, PassInputs, StepId, close_held_run, effect, gated_task, mark, open_heads, reconcile,
};
use crate::Ctx;
use crate::pane_liveness::Snapshot;

pub(super) fn reconcile_heads(
    ctx: &Ctx,
    pass: &mut PassInputs<'_>,
    board: &Board,
    panes: &HashMap<String, PaneInfo>,
    snapshot: Snapshot<'_>,
) -> Result<Vec<Change>> {
    let (now, scope) = (pass.now, pass.scope);
    let mut changes = Vec::new();
    let in_scope: Vec<&TaskHead> = board
        .heads
        .iter()
        .filter(|head| scope.includes(head.project.as_deref()))
        .collect();
    // Each task's head is read afresh: gate collection above may have just recorded a result.
    for &head in &in_scope {
        changes.extend(gated_task(
            ctx,
            pass.gate(),
            StepId::Obligations,
            head,
            || crate::obligations_sync::reconcile_one(ctx, &head.id, now, Some(panes)),
        )?);
    }
    let mut native: BTreeMap<_, _> = ctx
        .store
        .unfinished_native_heads()?
        .into_iter()
        .map(|head| (head.id.clone(), head.detach()))
        .collect();
    // Decided on this pass's board, so a pass that runs no head skips the open-head read.
    let decided: BTreeMap<String, bool> = in_scope
        .iter()
        .map(|head| (head.id.clone(), pass.gate().task(StepId::HeadLoop, head)))
        .collect();
    if decided.is_empty() || decided.values().any(|run| *run) {
        for head in scope.heads(open_heads(ctx)?) {
            let run = match decided.get(&head.id) {
                Some(run) => *run,
                None => pass.gate().task(StepId::HeadLoop, &head),
            };
            if !run || !crate::session_scope::owns_head(ctx, &head)? {
                continue;
            }
            let before = mark(ctx, pass.gate())?;
            let found = head_steps(ctx, pass, &head, &mut native, panes, snapshot)?;
            let acted = effect(ctx, before, found.len())?;
            pass.gate().finished_task(StepId::HeadLoop, &head.id, acted);
            changes.extend(found);
        }
    }
    // Indexed owed work survives terminal status and retired planning/review containers.
    for (_, head) in native {
        if scope.includes(head.project.as_deref()) && crate::session_scope::owns_head(ctx, &head)? {
            gated_task(ctx, pass.gate(), StepId::NativeLeftover, &head, || {
                crate::agent::hooks::sync::task_id(ctx, &head.id, now).map(drop)
            })?;
        }
    }
    Ok(changes)
}

/// Row 22 for one owned open head: plan, review and native hook state, then the worker run.
fn head_steps(
    ctx: &Ctx,
    pass: &PassInputs<'_>,
    head: &TaskHead,
    native: &mut BTreeMap<String, TaskHead>,
    panes: &HashMap<String, PaneInfo>,
    snapshot: Snapshot<'_>,
) -> Result<Vec<Change>> {
    let now = pass.now;
    let mut changes = Vec::new();
    crate::plan_pilot::reconcile(ctx, &head.to_shell_task()?, snapshot, now)?;
    let reconciled = ctx.store.head(&head.id)?;
    if reconciled.status != head.status {
        changes.push(Change {
            task: head.id.clone(),
            from: head.status,
            to: reconciled.status,
            note: reconciled
                .last_plan_run
                .as_ref()
                .and_then(|r| r.pending_reply.clone())
                .unwrap_or_else(|| "planner agent state changed".into()),
            stage: None,
        });
    }
    let current = reconciled.to_shell_task()?;
    if let Some(change) = crate::launch_exit::plan_review(ctx, &current, snapshot, now)? {
        changes.push(change);
    }
    crate::plan_acceptance::reconcile_exit(ctx, &current)?;
    if let Some(change) =
        crate::reviewer::reconcile_review_sampled_at(ctx, &head.id, now, snapshot)?
    {
        changes.push(change);
    }
    // One hook call per task holding live corral-pi work; other tasks never reach it.
    let (head, agent) = if native.remove(&head.id).is_some() {
        let synced = crate::agent::hooks::sync::task_id(ctx, &head.id, now)?;
        (ctx.store.head(&head.id)?, synced)
    } else {
        (ctx.store.head(&head.id)?, Default::default())
    };
    let shell = head.to_shell_task()?;
    let agent_healthy = agent.current_worker_healthy(&shell);
    close_held_run(ctx, &head, panes, now)?;
    if !crate::plan_pilot::handles(&shell) && head.status.holds_slot() && head.launch.is_none() {
        if let Some(change) = crate::provider_stall::advance_agent(ctx, &head, panes, now)? {
            changes.push(change);
            return Ok(changes);
        }
        if let Some(change) = reconcile(
            ctx,
            &head,
            snapshot,
            now,
            pass.processes,
            agent_healthy,
            agent.attention.as_deref(),
        )? {
            changes.push(change);
        }
    }
    Ok(changes)
}
