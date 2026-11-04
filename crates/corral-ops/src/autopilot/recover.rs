//! Explicit, round-free recovery of identifiable infrastructure escalations.
use anyhow::{Context, Result, ensure};
use corral_core::{AutoStage, AutopilotState, RunOutcome, Status, TaskRef as _};
use serde_json::json;

use crate::{Ctx, verification};

fn known_lock(reason: &str) -> bool {
    let Some(reason) = reason
        .strip_prefix("integration preflight error: ")
        .or_else(|| reason.strip_prefix("integration preflight infrastructure error: "))
    else {
        return false;
    };
    ["Git operation ongoing: ", "Git lock contention: "]
        .iter()
        .any(|prefix| {
            reason.strip_prefix(prefix).is_some_and(|marker| {
                let marker = marker.split(" (").next().unwrap_or(marker);
                matches!(
                    marker,
                    "index.lock"
                        | "HEAD.lock"
                        | "packed-refs.lock"
                        | "MERGE_HEAD"
                        | "CHERRY_PICK_HEAD"
                        | "REVERT_HEAD"
                        | "BISECT_LOG"
                        | "rebase-apply"
                        | "rebase-merge"
                        | "sequencer"
                ) || (marker.starts_with("refs/heads/") && marker.ends_with(".lock"))
            })
        })
}

/// Return a finished run to Review without spending a change round. A retained typed
/// wait is deliberately left in place: the autopilot clears it only after markers clear.
pub fn recover_infrastructure(ctx: &Ctx, id: &str) -> Result<()> {
    let _lock = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("autopilot is advancing this task; retry recovery")?;
    ctx.store.dispatch_update_head(id, |store, task| {
        let waiting = task.status == Status::Review && task.head().infra_wait.is_some();
        ensure!(
            waiting || task.status == Status::NeedsMe,
            "task is not an infrastructure wait or escalation"
        );
        let run = task
            .head()
            .current_run_head()?
            .context("task has no current run")?
            .into_owned();
        ensure!(
            run.outcome == Some(RunOutcome::Done),
            "task has no finished run"
        );
        if !waiting {
            let pilot = run
                .autopilot
                .as_ref()
                .context("task has no autopilot escalation")?;
            ensure!(
                pilot.stage == AutoStage::Escalated
                    && pilot.reason.as_deref().is_some_and(known_lock),
                "escalation is not an identifiable infrastructure wait"
            );
            ensure!(
                run.review
                    .as_ref()
                    .is_none_or(|review| review.verdict.is_none()),
                "a completed review cannot be discarded by infrastructure recovery"
            );
        }
        let current = Ctx {
            store: store.clone(),
            ..ctx.clone()
        };
        ensure!(
            verification::gate_passed(&current, task.head(), &run)?,
            "a fresh passing gate is required for infrastructure recovery"
        );
        let lost = run
            .review
            .as_ref()
            .is_some_and(|review| review.verdict.is_none() && review.process_lost);
        if !waiting || lost {
            crate::reviewer::supersede_review_head(task, corral_core::clock::now())?;
            let current_run = task.current_run_mut().context("current run disappeared")?;
            current_run.autopilot = Some(AutopilotState {
                stage: AutoStage::Reviewing,
                reason: None,
                sha: crate::gate::run_head_sha(&run),
                at: corral_core::clock::now(),
            });
            task.status = Status::Review;
        }
        store.log_event(Some(id), "infra_recovery", json!({"run":run.id}))?;
        Ok(true)
    })?;
    Ok(())
}
