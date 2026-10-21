//! A reviewer whose provider failed continues as a new review attempt on the next approved
//! candidate of the same reviewer profile, for the same candidate SHA and review contract.
//! Partial notes are kept as context; they never become a verdict.

use anyhow::{Context, Result};
use corral_core::{AutoStage, AutopilotState, Run, RunOutcome, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use super::{finish_review, review_notes_path};
use crate::cleanup::{Cleanup, cleanup};
use crate::{Change, Ctx};

pub(crate) fn continue_review(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    child: &Run,
    reason: &str,
    now: Timestamp,
) -> Result<Vec<Change>> {
    let notes = review_notes_path(ctx, &task.id, &run.id);
    let kept = notes.with_file_name(format!("{}-{}-interrupted-notes.md", task.id, child.id));
    if notes.is_file() {
        std::fs::rename(&notes, &kept)
            .with_context(|| format!("keeping interrupted notes at {}", kept.display()))?;
    }
    ctx.store.update(&task.id, |t| {
        if let Some(interrupted) = t
            .current_run_mut()
            .filter(|r| r.id == run.id)
            .and_then(|r| r.review_runs.iter_mut().find(|c| c.id == child.id))
            .filter(|c| c.outcome.is_none())
        {
            interrupted.outcome = Some(RunOutcome::Interrupted);
        }
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "review_interrupted",
        json!({"run": run.id, "review_run": child.id, "reviewer": child.agent,
               "reason": reason, "notes": kept.is_file().then_some(&kept)}),
    )?;
    // The interrupted attempt's reviewer profile, its purpose and instructions, continue.
    match super::review_run_automatic(ctx, &task.id, Some(&child.agent)) {
        Ok(record) => {
            ctx.store.log_event(
                Some(&task.id),
                "availability_switched",
                json!({"run": run.id, "from_review": child.id, "reviewer": record.reviewer,
                       "role": corral_core::ProfileRole::ImplementationReview, "reason": reason}),
            )?;
            Ok(Vec::new())
        }
        Err(error) => {
            let note = format!("{reason}; fallback review blocked: {error:#}");
            crate::provider_stall::block_continuation(
                ctx,
                &crate::provider_stall::agent_key(&task.id, child),
                &note,
            )?;
            Ok(escalate(ctx, task, run, &note, now)?.into_iter().collect())
        }
    }
}

fn escalate(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    note: &str,
    now: Timestamp,
) -> Result<Option<Change>> {
    let saved = ctx.store.try_update(&task.id, |t| {
        if t.status != Status::Review {
            return Ok(false);
        }
        let Some(parent) = t.current_run_mut().filter(|r| r.id == run.id) else {
            return Ok(false);
        };
        if parent.review.as_ref().is_some_and(|r| r.verdict.is_some()) {
            return Ok(false);
        }
        finish_review(ctx, parent, now);
        parent.blocked_excerpt = Some(note.to_owned());
        parent.autopilot = Some(AutopilotState {
            stage: AutoStage::Escalated,
            reason: Some(note.to_owned()),
            sha: parent.review.as_ref().map(|r| r.sha.clone()),
            at: now,
        });
        t.status = Status::NeedsMe;
        Ok(true)
    })?;
    let Some(saved) = saved else {
        return Ok(None);
    };
    cleanup(ctx, &saved, Cleanup::Round);
    if let Err(error) = ctx
        .herdr
        .notify(&format!("{} needs you", task.id), note, true)
    {
        crate::messages::report(
            Some(&task.id),
            "notification",
            format!("review fallback notification failed: {error:#}"),
        );
    }
    ctx.store.log_event(
        Some(&task.id),
        "review_escalated",
        json!({"run": run.id, "reason": note}),
    )?;
    Ok(Some(Change {
        task: task.id.clone(),
        from: Status::Review,
        to: Status::NeedsMe,
        note: note.to_owned(),
        stage: Some(AutoStage::Escalated),
    }))
}
