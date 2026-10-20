//! Bounded stronger review: one automatically routed routine attempt may hand off to one
//! strong attempt. Pinned reviewers and already-strong attempts escalate to the owner instead.
use anyhow::{Context, Result, bail};
use corral_core::{ReviewCapability, Status, Verdict};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use super::{current_run, review_parent, supersede_review, verdict_bound};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum StrongerOutcome {
    /// `started` is None when admission or authority deferred the strong attempt.
    Rerouted {
        superseded: String,
        started: Option<String>,
        pending_reason: Option<String>,
    },
    Escalated {
        reason: String,
    },
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
    let pending = |review: &corral_core::ReviewRecord| {
        review.verdict.is_none()
            && review.identity.as_ref().map(|i| i.attempt.as_str()) == Some(attempt)
    };
    let review = run
        .review
        .as_ref()
        .filter(|r| pending(r))
        .with_context(|| format!("stale review: attempt {attempt} is not the pending review"))?;
    let route = review
        .route
        .as_deref()
        .context("this review has no recorded route; record escalate instead")?;
    let refusal = if route.profile_source != "auto_mapping" {
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
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    let saved = ctx.store.update(id, |t| {
        let parent = review_parent(t, &run.id)?;
        let review = parent
            .review
            .as_mut()
            .filter(|r| pending(r))
            .context("review changed before the stronger-review request")?;
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
    let (started, pending_reason) = match super::review_run_automatic(ctx, id, None) {
        Ok(_) => (
            current_run(&ctx.store.get(id)?)?
                .review_runs
                .last()
                .map(|c| c.id.clone()),
            None,
        ),
        Err(error) => {
            crate::messages::report(
                Some(id),
                "review",
                format!("stronger review is pending admission: {error:#}"),
            );
            (None, Some(format!("{error:#}")))
        }
    };
    Ok(StrongerOutcome::Rerouted {
        superseded,
        started,
        pending_reason,
    })
}
