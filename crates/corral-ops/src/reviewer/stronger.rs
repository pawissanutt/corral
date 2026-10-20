//! Bounded stronger review: one automatically routed routine attempt may hand off to one
//! strong attempt. Pinned reviewers and already-strong attempts escalate to the owner instead.
//! The replacement is started by the normal review launch (owner or autopilot), which closes
//! the superseded reviewer's pane first; the requesting reviewer never launches it itself.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{ReviewCapability, ReviewRecord, Run, RunOutcome, Status, Task, Verdict};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use super::{PM, callback, current_run, review_parent, supersede_review, verdict_bound};
use crate::{Ctx, verification};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StrongerOutcome {
    Rerouted { superseded: String },
    Escalated { reason: String },
}

/// The pane of a superseded dedicated reviewer that a replacement launch must close.
pub(crate) fn superseded_pane(run: &Run) -> Option<&str> {
    if run.review.is_some() {
        return None;
    }
    let child = run
        .review_runs
        .last()
        .filter(|c| c.outcome == Some(RunOutcome::Superseded))?;
    let review = child.review.as_ref().filter(|r| r.reviewer != PM)?;
    review
        .pane
        .as_deref()
        .or(child.herdr.as_ref().map(|h| h.pane_id.as_str()))
}

fn current(ctx: &Ctx, task: &Task, run: &Run, review: &ReviewRecord) -> Result<()> {
    ensure!(
        corral_git::is_clean(&run.worktree)?,
        "stale review: worktree {} has uncommitted changes",
        run.worktree.display()
    );
    let mut changes = verification::review_changes(ctx, task, run, review)?;
    if changes.is_empty()
        && let Some(route) = review.route.as_deref()
    {
        changes = crate::review_packet::route_changes(ctx, task, run, review, route, true)?;
    }
    ensure!(
        changes.is_empty(),
        "stale review: {}",
        serde_json::to_string(&changes)?
    );
    Ok(())
}

pub fn request_stronger_review(
    ctx: &Ctx,
    id: &str,
    attempt: &str,
    notes: &str,
) -> Result<StrongerOutcome> {
    if notes.trim().is_empty() {
        bail!("a stronger-review request needs notes with concrete evidence");
    }
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
        bail!(
            "{} is {}; stronger review needs a pending review",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    let review = run
        .review
        .as_ref()
        .filter(|r| r.verdict.is_none() && callback::matches(&run, r, attempt))
        .with_context(|| format!("stale review: attempt {attempt} is not the pending review"))?;
    let route = review
        .route
        .as_deref()
        .context("this review has no recorded route; record escalate instead")?;
    let refusal = if !route.profile_source.starts_with("auto_") {
        Some(format!(
            "reviewer '{}' was selected by {}; switching it needs an owner decision",
            route.profile, route.profile_source
        ))
    } else if route.requirement.capability == ReviewCapability::Strong
        || route.escalated_from.is_some()
    {
        Some("this attempt is already the strong review; escalation is bounded to one step".into())
    } else {
        None
    };
    if let Some(reason) = refusal {
        verdict_bound(
            ctx,
            id,
            Verdict::Escalate,
            &format!(
                "{}\n\nStronger review requested: {reason}.",
                notes.trim_end()
            ),
            Some(attempt),
        )?;
        return Ok(StrongerOutcome::Escalated { reason });
    }
    current(ctx, &task, &run, review)?;
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    let saved = ctx.store.update(id, |t| {
        let snapshot = t.clone();
        let parent = snapshot
            .current_run()
            .filter(|r| r.id == run.id)
            .context("run changed before the stronger-review request")?;
        let pending = parent
            .review
            .as_ref()
            .filter(|r| r.verdict.is_none() && callback::matches(parent, r, attempt))
            .context("review changed before the stronger-review request")?;
        current(ctx, &snapshot, parent, pending)?;
        let parent = review_parent(t, &run.id)?;
        let review = parent
            .review
            .as_mut()
            .context("review disappeared before the stronger-review request")?;
        review.notes = Some(notes.trim_end().to_string());
        if let Some(route) = review.route.as_mut() {
            route.stronger_request = Some(notes.trim().to_string());
        }
        supersede_review(ctx, parent, Timestamp::now());
        Ok(())
    })?;
    let superseded = current_run(&saved)?
        .review_runs
        .last()
        .map(|c| c.id.clone())
        .context("superseded review attempt missing")?;
    ctx.store.log_event(
        Some(id),
        "review_stronger_requested",
        json!({"run": run.id, "superseded": superseded, "profile": route.profile,
            "contract": route.requirement.contract, "notes": notes.trim_end()}),
    )?;
    crate::messages::report(
        Some(id),
        "review",
        format!("{superseded} asked for a stronger review; the next review launch routes it"),
    );
    Ok(StrongerOutcome::Rerouted { superseded })
}
