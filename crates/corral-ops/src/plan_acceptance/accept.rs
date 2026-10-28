//! Final acceptance: one authorized transaction per Plan, landed exactly once.
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    AGGREGATE_RUN, AggregateAcceptance, AggregateAttempt, AggregateLandingState, AggregateStrategy,
    OperatingActionKind, PlanDeliveryTarget, Status, Store, Task, TaskHead, TaskMut as _,
    TaskRef as _,
};
use serde_json::json;

use super::{capture, current_head, ensure_fresh, evidence_failures_head, gate, scenario_failures};
use crate::Ctx;
use crate::operating::Origin;

/// Accept the current attempt and land it. Owner acceptance is the default and still needs the
/// configured reviewer's current completed aggregate review; `automatic` acts only under a
/// confirmed operating grant with `aggregate_accept` and an independent approving review. Integration Plans fast-forward main to the checked candidate;
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
    let _action =
        automatic.then(|| corral_core::work::action(id, OperatingActionKind::AcceptAggregate));
    // The tracker is read as a head: the current attempt is its projection with its current
    // review, and the acceptance reads nothing older.
    let tracker = ctx.store.head(id)?.shell_task()?;
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
    let tracker_head = ctx.store.head(id)?;
    let tracker = tracker_head.shell_task()?;
    let chosen_head = current_head(&tracker_head, attempt)?.clone();
    let chosen = current_attempt(&tracker, attempt)?.clone();
    let store = ctx.store.reopen()?;
    check(ctx, &store, &tracker_head, &tracker, &chosen, automatic)?;
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
            &tracker_head,
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
        &format!("{}-{}", chosen.id, jiff::Timestamp::now().as_nanosecond()),
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
        at: corral_core::clock::now(),
        strategy,
        before: chosen.code.then(|| main.clone()),
        after: chosen.code.then(|| head.clone()),
        state: AggregateLandingState::Prepared,
    };
    let submitted = crate::operating::submit_head(
        ctx,
        &authorization,
        |store, task| {
            let fresh = Ctx {
                store: store.clone(),
                ..ctx.clone()
            };
            // The attempt projection compares its review facts too, so a review child edited
            // and edited back (A->B->A) refuses as a genuine change does.
            ensure!(
                current_head(task.head(), &chosen.id)? == &chosen_head,
                "aggregate attempt changed before acceptance"
            );
            let shell = task.shell_task()?;
            check(&fresh, store, task.head(), &shell, &chosen, automatic)?;
            task.last_aggregate_attempt_mut()
                .context("aggregate attempt missing")?
                .acceptance = Some(prepared.clone());
            Ok(())
        },
        || match &landing {
            Some((branch, checkout)) => {
                ctx.store.submission_effect_barrier("effect_entered");
                let landed =
                    corral_git::land_checked(&repo, checkout, branch, "main", &head, &main);
                ctx.store.submission_effect_barrier("effect_returned");
                landed
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

/// The current attempt of a head's shell: its projection with only the current review.
fn current_attempt<'a>(tracker: &'a Task, attempt: &str) -> Result<&'a AggregateAttempt> {
    super::current(tracker, attempt)
}

/// Every proof the attempt needs, recomputed against the current board, Git and policy.
/// `tracker` is `head`'s shell, `attempt` its current attempt.
fn check(
    ctx: &Ctx,
    store: &Store,
    head: &TaskHead,
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
        head,
        &attempt.id,
        Some(&attempt.identity.environment),
    )?;
    ensure_fresh(attempt, &candidate)?;
    if attempt.code {
        let policy = store.config.task_review_policy(tracker);
        crate::verification::require_gate(&policy, tracker)?;
        let checkout = capture::repo(store, tracker)?
            .join(".worktrees")
            .join(format!("{}-verify", tracker.id));
        ensure!(
            gate::passed(ctx, attempt, &policy.gate, &checkout)?,
            "aggregate gate has not passed for {}; a local task PASS cannot substitute",
            attempt.id
        );
    }
    let mut failures = scenario_failures(tracker, attempt);
    failures.extend(evidence_failures_head(ctx, head, attempt));
    ensure!(
        failures.is_empty(),
        "aggregate scenarios are not all passing: {}",
        failures.join("; ")
    );
    super::review::require(store, tracker, attempt, automatic)?;
    Ok(())
}

/// Resolve a previous transaction before starting another. Never merges a second time.
fn recover(ctx: &Ctx, id: &str, attempt: &str, repo: &Path) -> Result<Option<AggregateAcceptance>> {
    // Only the current attempt can carry an acceptance: verification refuses a new attempt
    // once one exists, and a release clears it.
    let tracker = ctx.store.head(id)?;
    let Some(accepted) = tracker
        .last_aggregate_attempt
        .as_ref()
        .filter(|attempt| attempt.acceptance.is_some())
    else {
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
    ctx.store.update_head(id, |task| {
        task.edit_aggregate_attempt(attempt, |stored| {
            if stored
                .acceptance
                .as_ref()
                .is_some_and(|acceptance| acceptance.state == AggregateLandingState::Prepared)
            {
                stored.acceptance = None;
            }
            Ok(())
        })
        .context("aggregate attempt missing")
    })?;
    Ok(())
}

fn finalize(ctx: &Ctx, id: &str, attempt: &str, after: &str) -> Result<AggregateAcceptance> {
    let mut landed = None;
    ctx.store.update_head(id, |task| {
        let mut detail = serde_json::Value::Null;
        task.edit_aggregate_attempt(attempt, |stored| {
            let code = stored.code;
            let acceptance = stored
                .acceptance
                .as_mut()
                .context("prepared acceptance missing")?;
            acceptance.state = AggregateLandingState::Landed;
            if code {
                acceptance.after = Some(after.into());
            }
            detail = json!({"attempt": attempt, "strategy": acceptance.strategy, "before": acceptance.before, "after": acceptance.after, "automatic": acceptance.automatic, "authorization": acceptance.authorization, "identity": stored.identity});
            landed = Some(acceptance.clone());
            Ok(())
        })
        .context("aggregate attempt missing")?;
        task.status = Status::Done;
        task.queued = false;
        task.dispatch_plan = None;
        ctx.store.log_event(Some(id), "plan_accepted", detail)?;
        Ok(())
    })?;
    let acceptance = landed.context("acceptance finalization produced no result")?;
    Ok(acceptance)
}
