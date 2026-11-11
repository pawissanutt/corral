//! Existing manual receipt requirements shared by ordinary and recovered acceptance.
use crate::review::landing_gate_fresh;
use crate::{Ctx, verification};
use anyhow::{Context, Result, bail};
use corral_core::{CoworkReview, GateResult, ReviewPolicy, Reviewer, RunHead, TaskRef, Verdict};

/// A review must approve the candidate: the policy names a reviewer, or the owner finished the
/// current co-work run with review required, which binds whatever the reviewer resolves to.
pub(crate) fn review_needed<T: TaskRef + ?Sized>(policy: &ReviewPolicy, task: &T) -> bool {
    policy.reviewer != Reviewer::None
        || ((task.cowork_run()).is_some_and(|cowork| cowork.review != CoworkReview::None)
            && !task.review_waived())
}

/// The reduced tier an approval is bound to while the current receipt is a failed full one.
fn quick_approval_on_full_fail(run: &RunHead) -> Option<&str> {
    let gate = run
        .gate
        .as_ref()
        .filter(|g| matches!(g.result, GateResult::Fail | GateResult::Stale))?;
    gate.identity.as_ref().filter(|i| i.gate_tier.is_none())?;
    let review = run.review.as_ref()?;
    let binding = review.identity.as_ref()?.gate_tier.as_deref()?;
    (review.verdict == Some(Verdict::Approve)).then_some(binding.tier.as_str())
}

/// Owner acceptance can override a completed check's outcome, but not its candidate identity.
/// Keep receipts as stale evidence after a rebase; retries must refresh every configured check.
pub(crate) fn require_manual_receipts<T: TaskRef + ?Sized>(ctx: &Ctx, task: &T) -> Result<()> {
    let run = &*task.current_run_head()?.context("run disappeared")?;
    let policy = verification::policy(ctx, task)?;
    if task.reviewer().is_some()
        || ctx
            .store
            .reopen()?
            .config
            .has_review_policy(task.project().as_deref(), *task.review())
    {
        verification::require_gate(&policy, task)?;
    }
    if !policy.gate.is_empty()
        && !match &run.gate {
            Some(gate) if gate.result != GateResult::Running => {
                landing_gate_fresh(ctx, task, run, gate)?
                    && crate::gate::gate_log_valid(ctx, task, run)?
            }
            _ => false,
        }
    {
        bail!(
            "stale_retry: gate missing, pending or stale; run `corral gate {}` then `corral approve {}`",
            task.id(),
            task.id()
        );
    }
    if review_needed(&policy, task)
        && (!verification::review_fresh(ctx, task, run)?
            || run.review.as_ref().and_then(|r| r.verdict).is_none())
    {
        if policy.reviewer == Reviewer::None {
            bail!(
                "review_required: {0} was finished with review required, but no reviewer resolves for it; run `corral review-run {0} --reviewer <profile>`, then `corral approve {0}` after its verdict",
                task.id()
            );
        }
        if let Some(tier) = quick_approval_on_full_fail(run) {
            bail!(
                "stale_retry: the approval is bound to the {tier} receipt and promotion needs a full PASS, but the full landing gate failed; run `corral review-run {}` to review the failed full receipt, then `corral approve {}` after its verdict",
                task.id(),
                task.id()
            );
        }
        bail!(
            "stale_retry: semantic review missing, pending or stale; run `corral review-run {}` then `corral approve {}` after its verdict",
            task.id(),
            task.id()
        );
    }
    Ok(())
}
