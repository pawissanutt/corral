//! Pending review liveness, independent of the automatic acceptance policy.

use std::collections::HashMap;

use anyhow::{Context, Result};
use corral_core::{AutoStage, AutopilotState, ReviewRecord, RunOutcome, Status, Task};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::{finish_review, review_notes_path};
use crate::cleanup::{Cleanup, cleanup};
use crate::{Change, Ctx};

const FAILURE_PREFIX: &str = "reviewer verdict failed: ";

/// Keep callback errors on the pending review, not on a replacement or a completed verdict.
pub(super) fn record_failure(
    ctx: &Ctx,
    task: &Task,
    review: &ReviewRecord,
    error: &anyhow::Error,
) -> Result<()> {
    let run = task.current_run().context("review run disappeared")?;
    ctx.store.try_update(&task.id, |current| {
        if current.status != Status::Review {
            return Ok(false);
        }
        let Some(parent) = current.current_run_mut().filter(|r| {
            r.id == run.id
                && r.review.as_ref().is_some_and(|current| {
                    // Launch can attach the pane while a fast callback is failing.
                    current.verdict.is_none()
                        && current.at == review.at
                        && current.identity == review.identity
                        && current.sha == review.sha
                        && current.reviewer == review.reviewer
                })
        }) else {
            return Ok(false);
        };
        parent.blocked_excerpt = Some(format!("{FAILURE_PREFIX}{error:#}"));
        Ok(true)
    })?;
    Ok(())
}

/// Called by sync even when autopilot is disabled. `panes` is one authoritative herdr inventory;
/// `now` is injected, and idle debounce is persisted so restarts cannot reset the deadline.
pub fn reconcile_review_at(
    ctx: &Ctx,
    id: &str,
    now: Timestamp,
    panes: &HashMap<String, PaneInfo>,
) -> Result<Option<Change>> {
    reconcile_review_sampled_at(
        ctx,
        id,
        now,
        crate::pane_liveness::Snapshot::injected(panes),
    )
}

pub(crate) fn reconcile_review_sampled_at(
    ctx: &Ctx,
    id: &str,
    now: Timestamp,
    snapshot: crate::pane_liveness::Snapshot<'_>,
) -> Result<Option<Change>> {
    let panes = snapshot.panes;
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
        return Ok(None);
    }
    if !crate::session_scope::owns_task(ctx, &task)? {
        return Ok(None);
    }
    if panes.is_empty() {
        crate::pane_liveness::unknown("herdr returned an empty pane inventory");
        return Ok(None);
    }
    let Some(run) = task.current_run() else {
        return Ok(None);
    };
    let Some(review) = run.review.as_ref().filter(|r| r.verdict.is_none()) else {
        return Ok(None);
    };
    if crate::availability::review_continuation_due(ctx, &task, run)? {
        return Ok(None);
    }
    let timeout = ctx.store.config.sync.review_idle_after_secs;
    let window = SignedDuration::from_secs(i64::try_from(timeout).unwrap_or(i64::MAX));
    let activity_window = SignedDuration::from_secs(
        i64::try_from(ctx.store.config.sync.idle_after_secs).unwrap_or(i64::MAX),
    );
    let pane = review.pane.as_deref().and_then(|id| panes.get(id));
    let gone = crate::pane_liveness::gone(
        ctx,
        crate::pane_liveness::Sample {
            task: &task,
            pane: review.pane.as_deref(),
            started: review.at,
            seen: false,
            present: pane.is_some(),
            at: now,
            ticket: snapshot.ticket,
            count_miss: true,
        },
    )?;
    if pane.is_none() && !gone {
        return Ok(None);
    }
    let mut stalled = None;
    if let (Some(child), Some(pane_id), Some(info)) = (
        run.review_runs.last().filter(|c| c.outcome.is_none()),
        review.pane.as_deref(),
        pane,
    ) {
        match crate::provider_stall::advance_reviewer(
            ctx,
            id,
            child,
            review.reviewer == "pm",
            pane_id,
            info,
            (review.reviewer != "pm")
                .then(|| crate::availability::review_episode(id, run, &child.agent)),
            crate::availability::review_exclusion(ctx, &task, run),
            now,
        )? {
            // A selected fallback reviewer starts after the pane inventory is released.
            crate::provider_stall::Drive::Holding | crate::provider_stall::Drive::Fallback(_) => {
                return Ok(None);
            }
            crate::provider_stall::Drive::Escalated(reason) => stalled = Some(reason),
            crate::provider_stall::Drive::Quiet => {}
        }
    }
    let omo_active = run.review_runs.last().is_some_and(|child| {
        if !crate::omo::is_omo(child) {
            return false;
        }
        let agent = crate::omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
        crate::omo::last_activity(&agent, &child.worktree)
            .is_some_and(|at| at >= child.started && now.duration_since(at) < activity_window)
    });
    let idle = !omo_active
        && pane.is_some_and(|p| matches!(p.status, AgentStatus::Idle | AgentStatus::Done));
    // The worker is finished: this idle stretch belongs to its pending reviewer.
    let since = idle.then(|| run.idle_since.filter(|at| *at >= review.at).unwrap_or(now));
    let elapsed = since.is_some_and(|at| now.duration_since(at) >= window);
    let failure = run
        .blocked_excerpt
        .as_deref()
        .filter(|text| text.starts_with(FAILURE_PREFIX));
    let problem = if gone && crate::gate::worktree_head(run).as_deref() != Some(review.sha.as_str())
    {
        Some(
            "candidate changed during review; restart review for the current candidate".to_string(),
        )
    } else if gone {
        Some("closed without a verdict".to_string())
    } else if let Some(reason) = &stalled {
        Some(format!("stalled: {reason}"))
    } else if idle && review.identity.is_none() {
        Some(
            "is idle with a missing legacy review identity; restart review with the current binary"
                .into(),
        )
    } else if idle && failure.is_some() {
        failure.map(str::to_string)
    } else if elapsed {
        Some(format!(
            "has been idle for {timeout} seconds without a verdict"
        ))
    } else {
        None
    };
    let reason = problem.map(|problem| {
        format!(
            "the reviewer ({}) {problem}; saved notes: {}",
            review.reviewer,
            review_notes_path(ctx, id, &run.id).display()
        )
    });
    if since == run.idle_since && reason.is_none() {
        return Ok(None);
    }
    let saved = ctx.store.try_update(id, |current| {
        if !crate::session_scope::owns_task(ctx, current)? {
            return Ok(false);
        }
        if current.status != Status::Review {
            return Ok(false);
        }
        let Some(parent) = current.current_run_mut().filter(|r| r.id == run.id) else {
            return Ok(false);
        };
        if parent.review.as_ref() != Some(review)
            || parent.idle_since != run.idle_since
            || parent.blocked_excerpt != run.blocked_excerpt
        {
            return Ok(false);
        }
        parent.idle_since = since;
        if let Some(child) = parent.review_runs.last_mut() {
            child.idle_since = since;
            child.agent_state = Some(
                pane.map(|p| p.status.as_str())
                    .unwrap_or("gone")
                    .to_string(),
            );
        }
        if let Some(reason) = &reason {
            finish_review(ctx, parent, now);
            if stalled.is_some()
                && let Some(child) = parent.review_runs.last_mut()
            {
                child.outcome = Some(RunOutcome::Interrupted);
            }
            parent.blocked_excerpt = Some(reason.clone());
            parent.autopilot = Some(AutopilotState {
                stage: AutoStage::Escalated,
                reason: Some(reason.clone()),
                sha: Some(review.sha.clone()),
                at: now,
            });
            current.status = Status::NeedsMe;
        }
        Ok(true)
    })?;
    let (Some(saved), Some(reason)) = (saved, reason) else {
        return Ok(None);
    };
    cleanup(ctx, &saved, Cleanup::Round);
    if let Err(error) = ctx
        .herdr
        .notify(&format!("{} needs you", id), &reason, true)
    {
        crate::messages::report(
            Some(id),
            "notification",
            format!("review escalation notification failed: {error:#}"),
        );
    }
    if stalled.is_some() {
        ctx.store.log_event(
            Some(id),
            "review_interrupted",
            json!({"run": run.id, "reviewer": review.reviewer, "reason": reason}),
        )?;
    }
    ctx.store.log_event(
        Some(id),
        "review_escalated",
        json!({
            "run": run.id, "reason": reason,
            "notes": review_notes_path(ctx, id, &run.id),
        }),
    )?;
    Ok(Some(Change {
        task: id.to_string(),
        from: Status::Review,
        to: Status::NeedsMe,
        note: reason,
        stage: Some(AutoStage::Escalated),
    }))
}
