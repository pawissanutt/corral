//! What the owner may start for a submitted plan, and how to read a refusal of it. The action itself is
//! [`plan_review`](crate::plan_review()): the TUI runs the same path as `corral plan review <id>`.
use corral_core::{OperatingActionKind, PlanRunState, Task, Verdict};

use crate::{Ctx, operating, plan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanReviewOffer {
    /// The latest plan has no review attempt yet.
    Run,
    /// The last attempt failed to launch or lost its reviewer before a verdict.
    Retry,
}

/// The state-only part of [`plan_review_offer`]. A pending attempt is never offered a second one, like the CLI.
fn offer_for(task: &Task) -> Option<PlanReviewOffer> {
    if task.tracker || !task.children.is_empty() {
        return None;
    }
    let plan = task.plans.last().filter(|p| p.approval.is_none())?;
    if task.plan_runs.last().is_some_and(|r| {
        matches!(
            r.state,
            PlanRunState::Planning
                | PlanRunState::Changes
                | PlanRunState::Cancelled
                | PlanRunState::Approved
        )
    }) {
        return None;
    }
    match plan.reviews.last() {
        None => Some(PlanReviewOffer::Run),
        // Infrastructure failures did not review the snapshot. A substantive escalation,
        // however, needs an owner decision/revision, not another review of the same plan.
        Some(review)
            if review.verdict == Some(Verdict::Escalate)
                && review.notes.as_deref().is_some_and(|notes| {
                    notes.starts_with("Review launch failed:")
                        || notes == "plan reviewer disappeared before recording a verdict"
                }) =>
        {
            Some(PlanReviewOffer::Retry)
        }
        Some(_) => None,
    }
}

/// `Run` or `Retry` when `corral plan review <task>` would start an agent review now. An owner-reviewed plan has
/// no agent to start. A configuration that cannot name a reviewer still offers the action so the refusal is shown.
pub fn plan_review_offer(ctx: &Ctx, task: &Task) -> Option<PlanReviewOffer> {
    let offer = offer_for(task)?;
    let reviewer = plan::config(ctx).and_then(|config| plan::expected_reviewer(&config, task));
    (reviewer.as_deref().ok() != Some("owner")).then_some(offer)
}

/// Why automation will not start the review of a submitted plan by itself (paused, Assist without a grant, task
/// outside the grant). `None` when there is nothing waiting or automation will launch it.
pub fn plan_review_wait(ctx: &Ctx, task: &Task) -> Option<String> {
    (plan_review_offer(ctx, task)? == PlanReviewOffer::Run)
        .then(|| operating::require(&ctx.store, task, OperatingActionKind::PlanReview).err())
        .flatten()
        .map(|error| format!("{error:#}"))
}

/// A refusal that leaves the plan unchanged and the same request valid later: pane inventory under
/// reconciliation, a full review or machine slot, or a policy revision that changed under the
/// request (a fresh owner request re-authorizes against the current policy). Other authorization
/// refusals will not clear by asking again.
pub fn plan_review_retryable(error: &anyhow::Error) -> bool {
    const MARKERS: [&str; 5] = [
        "action policy revision changed",
        "pane inventory is being reconciled",
        "review process limit reached",
        "waiting: machine busy",
        "waiting for a slot",
    ];
    operating::transient_refusal(error)
        || error
            .chain()
            .any(|cause| MARKERS.iter().any(|m| cause.to_string().contains(m)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn classifies_transient_refusals_through_context_chains() {
        let busy = anyhow!("pane inventory is being reconciled; retry plan review");
        assert!(plan_review_retryable(&busy));
        assert!(plan_review_retryable(&anyhow!(
            "review process limit reached"
        )));
        assert!(plan_review_retryable(
            &anyhow!("waiting for a slot (3/3 tasks)").context("launching")
        ));
        assert!(!plan_review_retryable(&anyhow!(
            "plan review already pending"
        )));
        assert!(plan_review_retryable(&anyhow::Error::new(
            operating::AuthorizationRefused("action policy revision changed".into())
        )));
        assert!(!plan_review_retryable(&anyhow::Error::new(
            operating::AuthorizationRefused("automatic grant was revoked".into())
        )));
        assert!(plan_review_retryable(&anyhow::Error::new(
            operating::AuthorizationRefused("waiting: machine busy (load)".into())
        )));
    }
}
