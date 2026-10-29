//! Owner-visible automation state uses the same authority checks as the executors.
use anyhow::Result;
use corral_core::{AutoStage, OperatingActionKind, OperatingMode, RunOutcome, Status, Task};

use crate::Ctx;

/// The board and PM use only the current run and head fields for this check.
pub fn automation_blocker_head(ctx: &Ctx, head: &corral_core::TaskHead) -> Result<Option<String>> {
    let task = crate::pm_next::heads::shell(head)?;
    let kind = next_action_with(ctx, &task, |ctx, task, run| {
        let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
        let checked = crate::pm_next::heads::review_task(head, task, &sha)?;
        crate::verification::review_fresh(
            ctx,
            &checked,
            &corral_core::RunHead::of(
                checked
                    .current_run()
                    .ok_or_else(|| anyhow::anyhow!("review run missing"))?,
            )?,
        )
    })?;
    Ok(kind.and_then(|kind| {
        crate::operating::require(&ctx.store, &task, kind)
            .err()
            .map(|error| format!("{error:#}"))
    }))
}

/// [`automation_blocker`] of any task form, read from its fields and current run.
pub(crate) fn automation_blocker_ref<T: corral_core::TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
) -> Result<Option<String>> {
    let Some(kind) = next_action_head(ctx, task)? else {
        return Ok(None);
    };
    Ok(crate::operating::require(&ctx.store, task, kind)
        .err()
        .map(|error| format!("{error:#}")))
}

/// Why automatic review processing cannot take its next step. Collecting existing
/// gate receipts and recording reviewer verdicts remain available in Assist mode.
pub fn automation_blocker(ctx: &Ctx, task: &Task) -> Result<Option<String>> {
    let Some(kind) = next_action(ctx, task)? else {
        return Ok(None);
    };
    Ok(crate::operating::require(&ctx.store, task, kind)
        .err()
        .map(|error| format!("{error:#}")))
}

/// Landing-only consent can consume fresh verification, but cannot refresh stale
/// receipts. The executor and readbacks must choose the same prerequisite.
pub(crate) fn next_action(ctx: &Ctx, task: &Task) -> Result<Option<OperatingActionKind>> {
    next_action_with(ctx, task, |ctx, task, run| {
        crate::verification::review_fresh(ctx, task, &corral_core::RunHead::of(run)?)
    })
}

/// [`next_action`] of a head: its fields and current run.
pub(crate) fn next_action_head<T: corral_core::TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
) -> Result<Option<OperatingActionKind>> {
    if *task.status() != Status::Review || crate::plan_pilot::handles_ref(task)? {
        return Ok(None);
    }
    if task.reviewer().is_none()
        && !ctx
            .store
            .config
            .has_review_policy(task.project().as_deref(), *task.review())
    {
        return Ok(None);
    }
    let Some(run) = task
        .current_run_head()?
        .filter(|run| run.outcome == Some(RunOutcome::Done))
    else {
        return Ok(None);
    };
    action_of(ctx, task, &run, crate::verification::review_fresh)
}

fn next_action_with(
    ctx: &Ctx,
    task: &Task,
    review_fresh: impl FnOnce(&Ctx, &Task, &corral_core::Run) -> Result<bool>,
) -> Result<Option<OperatingActionKind>> {
    if task.status != Status::Review || crate::plan_pilot::handles(task) {
        return Ok(None);
    }
    if task.reviewer.is_none()
        && !ctx
            .store
            .config
            .has_review_policy(task.project.as_deref(), task.review)
    {
        return Ok(None);
    }
    let Some(run) = task
        .current_run()
        .filter(|run| run.outcome == Some(RunOutcome::Done))
    else {
        return Ok(None);
    };
    let run_head = corral_core::RunHead::of(run)?;
    action_of(ctx, task, &run_head, |ctx, task, _| {
        review_fresh(ctx, task, run)
    })
}

/// The action the current run `run` of `task` needs next.
fn action_of<T: corral_core::TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &corral_core::RunHead,
    review_fresh: impl FnOnce(&Ctx, &T, &corral_core::RunHead) -> Result<bool>,
) -> Result<Option<OperatingActionKind>> {
    let kind = match run.autopilot.as_ref().map(|state| state.stage) {
        Some(stage @ (AutoStage::Reviewed | AutoStage::Landing | AutoStage::Blocked)) => {
            let policy = ctx.store.config.task_review_policy(task);
            // Landing needs the landing tier's receipt, full on main by constant.
            let gate_stale = !policy.gate.is_empty()
                && !if stage == AutoStage::Reviewed {
                    crate::verification::gate_passed(ctx, task, run)?
                } else {
                    crate::review::landing_gate_passed(ctx, task, run)?
                };
            if gate_stale
                || (policy.reviewer != corral_core::Reviewer::None
                    && !review_fresh(ctx, task, run)?)
            {
                OperatingActionKind::Gate
            } else {
                OperatingActionKind::Accept
            }
        }
        Some(AutoStage::Reviewing) => OperatingActionKind::Review,
        Some(AutoStage::Changes) => OperatingActionKind::Repair,
        Some(AutoStage::Escalated | AutoStage::Done) => return Ok(None),
        None | Some(AutoStage::Gating) => OperatingActionKind::Gate,
    };
    Ok(Some(kind))
}

/// A kill switch being enabled does not mean a confirmed policy is effective.
pub fn automation_summary(ctx: &Ctx) -> Result<String> {
    let _hot = corral_core::work::HotScope::enter();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    let ctx = &frozen;
    crate::pm_next::heads::open(&ctx.store)?;
    let effective = ctx.store.effective_operating_policy()?;
    let mut reasons = effective.reasons.clone();
    if effective.paused {
        reasons.push("operating policy is paused".into());
    }
    if !ctx.store.config.autopilot_enabled() {
        reasons.push("legacy review automation is explicitly disabled".into());
    }
    let mode = match effective.mode {
        OperatingMode::Assist => "assist",
        OperatingMode::Autopilot => "autopilot",
    };
    if !reasons.is_empty() {
        return Ok(format!(
            "autopilot: blocked ({mode}): {}; existing receipts and verdicts still reconcile",
            reasons.join("; ")
        ));
    }
    Ok(format!(
        "autopilot: scoped ({mode}); grant {}; per-task authority and verification apply",
        effective.authorization.as_deref().unwrap_or("none")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_blocker_head_matches_full_task() {
        // Given review tasks with and without an automatic next step.
        let root = tempfile::tempdir().unwrap();
        let mut store = corral_core::Store::init(root.path()).unwrap();
        store.config.review.gate = Some(vec!["true".into()]);
        let ctx = Ctx::from_store(store);
        for status in [Status::Running, Status::Review, Status::NeedsMe] {
            let task = ctx.store.create("Blocker", None, status, "").unwrap();
            let run: corral_core::Run = serde_json::from_value(serde_json::json!({
                "id":"R1", "agent":"generic", "worktree":root.path(),
                "branch":"main", "base":"main", "base_sha":"fixture",
                "started":"2026-10-18T12:00:00Z", "outcome":"done"
            }))
            .unwrap();
            let task = ctx
                .store
                .update(&task.id, |task| {
                    task.runs.push(run);
                    Ok(())
                })
                .unwrap();
            // When the same authority check receives the head instead.
            let actual = automation_blocker_head(&ctx, &ctx.store.head(&task.id).unwrap()).unwrap();
            // Then it preserves the full task's result, including refusal prose.
            assert_eq!(actual, automation_blocker(&ctx, &task).unwrap());
        }
    }
}
