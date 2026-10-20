//! Final acceptance: one authorized transaction per Plan, landed exactly once.
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    AGGREGATE_RUN, AggregateAcceptance, AggregateAttempt, AggregateLandingState, AggregateStrategy,
    OperatingActionKind, PlanDeliveryTarget, Status, Store, Task, Verdict,
};
use jiff::Timestamp;
use serde_json::json;

use super::{capture, current, ensure_fresh, gate, scenario_failures};
use crate::Ctx;
use crate::operating::Origin;
use crate::verification::digest;

/// Accept the current attempt and land it. Owner acceptance is the default; `automatic`
/// acts only under a confirmed operating grant with `aggregate_accept` and an independent
/// approving aggregate review. Integration Plans fast-forward main to the checked candidate;
/// direct-to-main and no-code Plans complete without any Git change.
///
/// # Errors
/// Refuses stale proof, failing or missing aggregate scenarios, gate failures, unauthorized
/// automatic acceptance, and main movement outside a prepared landing.
pub fn plan_accept(
    ctx: &Ctx,
    id: &str,
    attempt: &str,
    automatic: bool,
) -> Result<AggregateAcceptance> {
    ctx.store.check_runtime()?;
    let tracker = ctx.store.get(id)?;
    ensure!(
        tracker.plan_acceptance_required(),
        "{} is not a tracker Plan with approved acceptance criteria",
        tracker.id
    );
    let repo = capture::repo(&ctx.store, &tracker)?;
    let _landing = corral_git::landing_lock(&repo)?;
    if let Some(done) = recover(ctx, id, attempt, &repo)? {
        return Ok(done);
    }
    let tracker = ctx.store.get(id)?;
    let chosen = current(&tracker, attempt)?.clone();
    let store = ctx.store.reopen()?;
    check(ctx, &store, &tracker, &chosen, automatic)?;
    let main = corral_git::rev_sha(&repo, "refs/heads/main")?;
    let head = chosen.identity.head.clone();
    let strategy = match (&chosen.target, chosen.code) {
        (_, false) => AggregateStrategy::NoCode,
        (PlanDeliveryTarget::Main, true) => AggregateStrategy::Direct,
        (PlanDeliveryTarget::Integration { .. }, true) if head == main => {
            AggregateStrategy::AlreadyOnMain
        }
        (PlanDeliveryTarget::Integration { .. }, true) => AggregateStrategy::FastForward,
    };
    let landing = if strategy == AggregateStrategy::FastForward {
        crate::hooks::reconcile_locked(ctx, &repo)?;
        if let Some(reason) = crate::hooks::operational_blocker(
            ctx,
            tracker.project.as_deref().context("Plan has no project")?,
        )? {
            bail!("{reason}; resolve post-land operations before advancing main");
        }
        let PlanDeliveryTarget::Integration { branch } = &chosen.target else {
            bail!("fast-forward requires an integration target");
        };
        let candidate = capture::capture(
            ctx,
            &store,
            &tracker,
            &chosen.id,
            Some(&chosen.identity.environment),
        )?;
        Some((
            branch.clone(),
            candidate
                .checkout
                .context("integration checkout missing for landing")?,
        ))
    } else {
        None
    };
    let mut action = crate::operating::identity(
        id,
        AGGREGATE_RUN,
        OperatingActionKind::AcceptAggregate,
        &format!("{}-{}", chosen.id, Timestamp::now().as_nanosecond()),
    );
    action.candidate = Some(chosen.identity.clone());
    let origin = if automatic {
        Origin::Automatic
    } else {
        Origin::Owner
    };
    let authorization = crate::operating::authorize(ctx, action, origin)?;
    let prepared = AggregateAcceptance {
        authorization: authorization.id.clone(),
        automatic,
        by: if automatic {
            "operating-policy"
        } else {
            "owner"
        }
        .into(),
        at: Timestamp::now(),
        strategy,
        before: chosen.code.then(|| main.clone()),
        after: chosen.code.then(|| head.clone()),
        state: AggregateLandingState::Prepared,
    };
    let submitted = crate::operating::submit(
        ctx,
        &authorization,
        |store, task| {
            let fresh = Ctx {
                store: store.clone(),
                ..ctx.clone()
            };
            ensure!(
                current(task, &chosen.id)? == &chosen,
                "aggregate attempt changed before acceptance"
            );
            check(&fresh, store, task, &chosen, automatic)?;
            task.aggregate_attempts
                .last_mut()
                .context("aggregate attempt missing")?
                .acceptance = Some(prepared.clone());
            Ok(())
        },
        || match &landing {
            Some((branch, checkout)) => {
                corral_git::land_checked(&repo, checkout, branch, "main", &head, &main)
            }
            None => Ok(main.clone()),
        },
    );
    match submitted {
        Ok(after) => finalize(ctx, id, &chosen.id, &after),
        Err(error) => {
            if corral_git::rev_sha(&repo, "refs/heads/main").ok().as_ref() == Some(&main) {
                release(ctx, id, &chosen.id)?;
            }
            Err(error)
        }
    }
}

/// Every proof the attempt needs, recomputed against the current board, Git and policy.
fn check(
    ctx: &Ctx,
    store: &Store,
    tracker: &Task,
    attempt: &AggregateAttempt,
    automatic: bool,
) -> Result<()> {
    ensure!(
        !matches!(tracker.status, Status::Done | Status::Dropped),
        "{} is {}",
        tracker.id,
        tracker.status.label()
    );
    let candidate = capture::capture(
        ctx,
        store,
        tracker,
        &attempt.id,
        Some(&attempt.identity.environment),
    )?;
    ensure_fresh(attempt, &candidate)?;
    if attempt.code {
        let policy = store.config.task_review_policy(tracker);
        crate::verification::require_gate(&policy, tracker)?;
        ensure!(
            gate::passed(ctx, attempt, &policy.gate)?,
            "aggregate gate has not passed for {}; a local task PASS cannot substitute",
            attempt.id
        );
    }
    let failures = scenario_failures(tracker, attempt);
    ensure!(
        failures.is_empty(),
        "aggregate scenarios are not all passing: {}",
        failures.join("; ")
    );
    if automatic {
        let evidence = digest(&serde_json::to_vec(&attempt.evidence)?);
        ensure!(
            attempt.review.as_ref().is_some_and(|review| {
                review.verdict == Some(Verdict::Approve)
                    && !matches!(review.reviewer.as_str(), "owner" | "pm")
                    && review.identity.as_ref() == Some(&attempt.identity)
                    && review.evidence_digest.as_deref() == Some(evidence.as_str())
            }),
            "automatic Plan acceptance requires an independent approving aggregate review of {}",
            attempt.id
        );
    }
    Ok(())
}

/// Resolve a previous transaction before starting another. Never merges a second time.
fn recover(ctx: &Ctx, id: &str, attempt: &str, repo: &Path) -> Result<Option<AggregateAcceptance>> {
    let tracker = ctx.store.get(id)?;
    let Some(accepted) = tracker.aggregate_acceptance() else {
        return Ok(None);
    };
    let acceptance = accepted.acceptance.clone().context("acceptance missing")?;
    ensure!(
        accepted.id == attempt,
        "{} already has acceptance for {}",
        tracker.id,
        accepted.id
    );
    if acceptance.state == AggregateLandingState::Landed {
        return Ok(Some(acceptance));
    }
    if acceptance.strategy != AggregateStrategy::FastForward {
        release(ctx, id, attempt)?;
        return Ok(None);
    }
    let main = corral_git::rev_sha(repo, "refs/heads/main")?;
    if Some(&main) == acceptance.after.as_ref() {
        return finalize(ctx, id, attempt, &main).map(Some);
    }
    ensure!(
        Some(&main) == acceptance.before.as_ref(),
        "main moved outside the prepared Plan landing; refusing reconciliation"
    );
    release(ctx, id, attempt)?;
    Ok(None)
}

fn release(ctx: &Ctx, id: &str, attempt: &str) -> Result<()> {
    ctx.store.update(id, |task| {
        let stored = task
            .aggregate_attempts
            .iter_mut()
            .find(|stored| stored.id == attempt)
            .context("aggregate attempt missing")?;
        if stored
            .acceptance
            .as_ref()
            .is_some_and(|acceptance| acceptance.state == AggregateLandingState::Prepared)
        {
            stored.acceptance = None;
        }
        Ok(())
    })?;
    Ok(())
}

fn finalize(ctx: &Ctx, id: &str, attempt: &str, after: &str) -> Result<AggregateAcceptance> {
    let mut landed = None;
    let task = ctx.store.update(id, |task| {
        let stored = task
            .aggregate_attempts
            .iter_mut()
            .find(|stored| stored.id == attempt)
            .context("aggregate attempt missing")?;
        let code = stored.code;
        let acceptance = stored
            .acceptance
            .as_mut()
            .context("prepared acceptance missing")?;
        acceptance.state = AggregateLandingState::Landed;
        if code {
            acceptance.after = Some(after.into());
        }
        landed = Some(acceptance.clone());
        task.status = Status::Done;
        task.queued = false;
        task.dispatch_plan = None;
        Ok(())
    })?;
    let acceptance = landed.context("acceptance finalization produced no result")?;
    let identity = task
        .aggregate_attempts
        .iter()
        .find(|stored| stored.id == attempt)
        .map(|stored| &stored.identity);
    ctx.store.log_event(
        Some(id),
        "plan_accepted",
        json!({"attempt": attempt, "strategy": acceptance.strategy, "before": acceptance.before, "after": acceptance.after, "automatic": acceptance.automatic, "authorization": acceptance.authorization, "identity": identity}),
    )?;
    Ok(acceptance)
}
