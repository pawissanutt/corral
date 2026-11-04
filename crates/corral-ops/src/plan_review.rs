//! Separate plan review attempts. These never populate implementation review receipts.
use anyhow::{Context, Result, bail};
use corral_core::{
    OperatingActionKind, OperatingMode, PlanApproval, PlanRecord, PlanReview, PlanRunState, Task,
    TaskMut as _, TaskRef as _, Verdict,
};
use serde_json::json;

use crate::{Ctx, plan};

mod launch;
pub(crate) mod prepared;
use launch::review_with_origin;
pub(crate) use prepared::{has_prepared, retains};

fn current<'a>(task: &'a mut Task, id: &str) -> Result<&'a mut PlanRecord> {
    task.plans
        .last_mut()
        .filter(|p| p.id == id)
        .context("stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`")
}

/// End admission and reject callbacks before a pending attempt is abandoned.
pub(crate) fn terminalize_pending(plan: &mut PlanRecord, reason: &str) {
    for review in plan
        .reviews
        .iter_mut()
        .filter(|r| r.verdict.is_none() && !r.cancelled)
    {
        review.verdict = Some(Verdict::Escalate);
        review.notes = Some(reason.into());
        review.pending_reply = None;
        review.at = corral_core::clock::now();
    }
}

/// Close only a dedicated reviewer workspace, never the shared PM.
pub(crate) fn close_plan_review(ctx: &Ctx, plan: &PlanRecord) -> Result<()> {
    let Some(review) = plan.reviews.last().filter(|r| r.reviewer != "pm") else {
        return Ok(());
    };
    let Some(herdr) = &review.herdr else {
        return Ok(());
    };
    if ctx.herdr.pane_get(&herdr.pane_id)?.is_some() {
        ctx.herdr.pane_close(&herdr.pane_id)?;
    }
    if !ctx
        .herdr
        .pane_list()?
        .iter()
        .any(|p| p.workspace_id == herdr.workspace_id)
        && ctx
            .herdr
            .workspace_list()?
            .iter()
            .any(|(id, _)| *id == herdr.workspace_id)
    {
        ctx.herdr.workspace_close(&herdr.workspace_id)?;
    }
    Ok(())
}

/// The attempt ID the next plan review will use, including a retained preparation's.
pub(crate) fn next_plan_attempt<T: corral_core::TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
) -> Result<String> {
    Ok(match prepared::load(ctx, task.id())? {
        Some(retained) => retained.review.id,
        None => format!(
            "PR{}",
            task.last_plan_head()?.map_or(0, |p| p.facts.children) + 1
        ),
    })
}

pub fn plan_review(ctx: &Ctx, id: &str, reviewer: Option<&str>) -> Result<PlanReview> {
    review_with_origin(ctx, id, reviewer, crate::operating::Origin::Owner, None)
}

pub(crate) fn review_auto(ctx: &Ctx, id: &str) -> Result<PlanReview> {
    let _action = corral_core::work::action(id, OperatingActionKind::PlanReview);
    review_with_origin(ctx, id, None, crate::operating::Origin::Automatic, None)
}

pub(crate) fn review_recovery(
    ctx: &Ctx,
    id: &str,
    reviewer: &str,
    recovery: &corral_core::RecoveryRecord,
) -> Result<PlanReview> {
    review_with_origin(
        ctx,
        id,
        Some(reviewer),
        crate::operating::Origin::Owner,
        Some(recovery),
    )
}

pub(crate) fn review_forced(
    ctx: &Ctx,
    id: &str,
    recovery: &corral_core::RecoveryRecord,
) -> Result<PlanReview> {
    review_with_origin(
        ctx,
        id,
        None,
        crate::operating::Origin::Owner,
        Some(recovery),
    )
}

/// A suspended attempt owns its next action even when an older verdict remains
/// on the task. Never let that older verdict trigger revision or acceptance.
pub(crate) fn resume_prepared(ctx: &Ctx, task: &Task) -> Result<bool> {
    let Some(preparation) = prepared::load(ctx, &task.id)? else {
        return Ok(false);
    };
    preparation.check(task.plans.last().context("prepared plan disappeared")?)?;
    if crate::operating::is_automatic(&preparation.authorization) {
        review_auto(ctx, &task.id)?;
    }
    Ok(true)
}

pub struct PlanVerdict<'a> {
    pub plan: &'a str,
    /// None is an explicit owner verdict, never attributed to a launched agent.
    pub attempt: Option<&'a str>,
    pub verdict: Verdict,
    pub notes: &'a str,
}

pub fn plan_verdict(ctx: &Ctx, id: &str, input: &PlanVerdict<'_>) -> Result<PlanReview> {
    plan_verdict_with_evidence(ctx, id, input, &[])
}

/// [`plan_verdict`] that also promotes `evidence` (`--evidence PATH`) inside the verdict's
/// update; a refused set records no verdict.
pub fn plan_verdict_with_evidence(
    ctx: &Ctx,
    id: &str,
    input: &PlanVerdict<'_>,
    evidence: &[std::path::PathBuf],
) -> Result<PlanReview> {
    verdict_report_inner(ctx, id, input, evidence, &|_| Ok(())).map(|(review, _)| review)
}

pub fn plan_verdict_report(
    ctx: &Ctx,
    id: &str,
    input: &PlanVerdict<'_>,
) -> Result<(PlanReview, Vec<String>)> {
    verdict_report_checked(ctx, id, input, &|_| Ok(()))
}

pub fn plan_verdict_report_with_evidence(
    ctx: &Ctx,
    id: &str,
    input: &PlanVerdict<'_>,
    evidence: &[std::path::PathBuf],
) -> Result<(PlanReview, Vec<String>)> {
    verdict_report_inner(ctx, id, input, evidence, &|_| Ok(()))
}

pub(crate) fn verdict_report_checked(
    ctx: &Ctx,
    id: &str,
    input: &PlanVerdict<'_>,
    check: &dyn Fn(&Task) -> Result<()>,
) -> Result<(PlanReview, Vec<String>)> {
    verdict_report_inner(ctx, id, input, &[], check)
}

fn verdict_report_inner(
    ctx: &Ctx,
    id: &str,
    input: &PlanVerdict<'_>,
    evidence: &[std::path::PathBuf],
    check: &dyn Fn(&Task) -> Result<()>,
) -> Result<(PlanReview, Vec<String>)> {
    let _attribution = input
        .attempt
        .map(|_| ctx.store.attribute(corral_core::Actor::Reviewer));
    if input.notes.trim().is_empty() {
        bail!("plan verdict needs nonempty notes");
    }
    let before = ctx.store.get(id)?;
    let roots = crate::reviewer::plan_roots(ctx, &before, input.plan, input.attempt)?;
    let roots: Vec<&std::path::Path> = roots.iter().map(std::path::PathBuf::as_path).collect();
    let evidence = crate::reviewer::Evidence::check(ctx, evidence, &roots)?;
    let staged = evidence.stage(ctx, id)?;
    let mut superseded = None;
    let saved = ctx.store.update(id, |task| {
        check(task)?;
        let target = crate::reviewer::current_target(task);
        let config = plan::config(ctx)?;
        let selected = task.plans.last().filter(|p| p.id == input.plan).context(
            "stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`",
        )?;
        plan::fresh(ctx, task, selected)?;
        let recovery = match input.attempt.or_else(|| selected.reviews.last().map(|r| r.id.as_str())) {
            Some(attempt) => crate::recovery::plan_review_record(ctx, task, input.plan, attempt)?,
            None => None,
        };
        if recovery.is_some() && input.attempt.is_none() && input.verdict == Verdict::Approve {
            bail!("recovery-bound plan requires trusted local owner confirmation through recover accept-plan");
        }
        let digest = match &recovery {
            Some(record) => record.preview.contract_digest.clone(),
            None => plan::policy_digest(&config, task)?,
        };
        let required = config.plan_required(task.project.as_deref(), task.plan_required);
        let mut mismatch = false;
        let mut authorized = true;
        let reviewer = match input.attempt {
            Some(attempt) => {
                let review = selected.reviews.last().filter(|r| r.id == attempt && r.verdict.is_none() && !r.cancelled)
                    .context("stale plan verdict: review attempt was replaced or already has a verdict")?;
                if review.policy_digest != digest {
                    bail!("stale plan verdict: reviewer policy changed; review again");
                }
                if let Some(route) = review.route.as_deref() {
                    let changes = crate::review_contracts::plan_route_changes(&config, task, route);
                    if !changes.is_empty() {
                        bail!("stale plan verdict: review route changed ({}); review again", changes.join(", "));
                    }
                    mismatch = route.capability_mismatch;
                    if let Some(receipt) = route.receipt.as_deref()
                        && !corral_core::plan_policy::plan_receipt_valid(&ctx.store, &config, task, receipt, &review.id)?
                    {
                        bail!("stale plan verdict: review receipt {receipt} no longer validates; review again");
                    }
                }
                authorized = if let Some(record) = &recovery {
                    matches!(&record.preview.proposed_effect, corral_core::RecoveryEffect::ReviewAttempt { reviewer, .. } if reviewer.profile == review.reviewer)
                        && review.herdr.is_some()
                } else {
                    corral_core::plan_policy::plan_reviewer_authorized(
                        &ctx.store, &config, task, review, &plan::expected_reviewer(&config, task)?)?
                        && (review.reviewer != "pm" || crate::availability::pm_plan_reviewer(ctx, &config, task).is_ok())
                };
                review.reviewer.clone()
            }
            None => {
                superseded = Some(selected.clone());
                "owner".into()
            }
        };
        let owner = input.attempt.is_none();
        let expected = if recovery.is_some() { "recovery-reviewer".into() } else { plan::expected_reviewer(&config, task)? };
        let reviewed = crate::recovery::plan_review_valid(ctx, task, selected).is_ok();
        let decisions = plan::owner_decisions(task, selected);
        let mut notes = input.notes.to_string();
        let verdict = match input.verdict {
            Verdict::Approve if owner && expected != "owner" && !reviewed => {
                bail!("independent review must approve before owner acceptance");
            }
            Verdict::Approve if !owner && !authorized => {
                notes.push_str("\nEscalated: unauthorized reviewer.");
                Verdict::Escalate
            }
            Verdict::Approve if mismatch && owner => {
                bail!("reviewer lacks the strong capability the plan contract requires");
            }
            Verdict::Approve if mismatch => {
                notes.push_str("\nEscalated: the reviewer lacks the strong capability the plan contract requires.");
                Verdict::Escalate
            }
            Verdict::Approve if decisions && owner && !plan::decisions_resolved(task, selected, input.notes) => {
                bail!("owner must resolve decisions explicitly in notes as `Resolved: <decision> => <resolution>`");
            }
            Verdict::Approve => Verdict::Approve,
            Verdict::Changes => Verdict::Changes,
            Verdict::Escalate => Verdict::Escalate,
        };
        if decisions && !owner {
            notes.push_str("\nOwner decisions remain; owner acceptance must record each as `Resolved: <decision> => <resolution>`.");
        }
        // Keep the snapshot's verdict without restarting its cancelled execution.
        let cancelled = task.plan_runs.iter().any(|r|
            Some(&r.id) == selected.run.as_ref() && r.state == PlanRunState::Cancelled);
        // Recording a review is always allowed, including while automatic work
        // is paused. Automatic acceptance is a separate fresh, scoped action.
        let accepted = verdict == Verdict::Approve && owner;
        if accepted {
            crate::plan_decompose::check_attachment(&ctx.store, task, input.plan)?;
        }
        // An old queued intent must not turn approval into an automatic implementation launch.
        if required || recovery.is_some() {
            task.queued = false;
        }
        if recovery.is_some() && accepted {
            task.dispatch_hold = true;
        }
        let p = current(task, input.plan)?;
        p.approval = accepted.then(|| PlanApproval {
            by: reviewer.clone(), at: corral_core::clock::now(), policy_digest: digest.clone(),
            authorization: None,
        });
        match input.attempt {
            Some(_) => {
                let review = p.reviews.last_mut().context("plan review missing")?;
                review.verdict = Some(verdict);
                review.notes = Some(notes);
                review.at = corral_core::clock::now();
            }
            None => {
                terminalize_pending(p, "Superseded by an owner decision.");
                p.reviews.push(PlanReview {
                cancelled: false,
                agent_host: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                id: format!("PR{}", p.reviews.len() + 1),
                reviewer,
                policy_digest: digest,
                verdict: Some(verdict),
                notes: Some(notes),
                at: corral_core::clock::now(),
                herdr: None,
                pending_reply: None,
                route: None,
                });
            }
        }
        let run_id = p.run.clone();
        if let Some(run) = task.plan_runs.iter_mut().find(|r|
            Some(&r.id) == run_id.as_ref() && r.state != PlanRunState::Cancelled) {
            run.state = match verdict {
                Verdict::Approve if accepted => PlanRunState::Approved,
                Verdict::Approve => PlanRunState::AwaitingOwner,
                Verdict::Changes => PlanRunState::Changes,
                Verdict::Escalate => PlanRunState::Escalated,
            };
        }
        let published = plan::published_design(task);
        if !cancelled && !published && verdict == Verdict::Approve && !accepted {
            let effective = ctx.store.effective_operating_policy()?;
            task.status = if recovery.is_some() || decisions || config.explicit_owner_acceptance(task, true)
                || !config.automation.authority.plan_accept
                || effective.mode == OperatingMode::Assist {
                corral_core::Status::NeedsMe
            } else {
                corral_core::Status::Review
            };
        } else if !cancelled && !published && verdict == Verdict::Approve && accepted && run_id.is_some() {
            task.status = corral_core::Status::Ready;
        }
        ctx.store.log_event(Some(&task.id), "plan_verdict",
            json!({"target":"plan","plan":input.plan,"review":task.plans.last().and_then(|p|p.reviews.last())}))?;
        if verdict == Verdict::Approve && !accepted {
            ctx.store.log_event(Some(&task.id), "plan_awaiting_owner",
                json!({"target":"plan","plan":input.plan,"reason":"awaiting owner approval"}))?;
        }
        staged.attach(ctx, task, &target)
    });
    let saved = match saved {
        Ok(saved) => saved,
        Err(error) => {
            staged.release(ctx)?;
            return Err(error);
        }
    };
    drop(staged);
    if let Some(review) = input.attempt {
        let dir = crate::scratch_reclaim::Attempt::PlanReview {
            task: id,
            plan: input.plan,
            review,
        };
        if let Err(error) = dir.close(ctx) {
            crate::messages::report(Some(id), "scratch", format!("{error:#}"));
        }
    }
    let p = saved.plans.last().context("plan disappeared")?;
    if let Some(previous) = superseded {
        close_plan_review(ctx, &previous)?;
    }
    close_plan_review(ctx, p)?;
    let mut refreshed = Vec::new();
    if p.approval.is_some() {
        refreshed = crate::plan_decompose::apply_refreshing(ctx, id, input.plan)?;
    } else if input.attempt.is_some()
        && p.reviews
            .last()
            .is_some_and(|r| r.verdict == Some(Verdict::Approve))
        && crate::operating::require(
            &ctx.store,
            &ctx.store.get(id)?,
            OperatingActionKind::AcceptPlan,
        )
        .is_ok()
    {
        // A refusal must not discard an already collected independent verdict.
        if let Err(error) = accept_auto(ctx, id, input.plan) {
            crate::messages::report(Some(id), "automatic plan acceptance", format!("{error:#}"));
        }
    }
    let review = p.reviews.last().cloned().context("plan verdict missing")?;
    let current = ctx.store.get(id)?;
    let cancelled = current
        .plan_runs
        .last()
        .is_some_and(|r| r.state == PlanRunState::Cancelled);
    if input.attempt.is_none()
        && crate::plan_dispatch::changes_requested(&current)
        && !cancelled
        && !current.tracker
        && current.status != corral_core::Status::Running
    {
        // The owner's verdict itself authorizes its revision, or a fresh planner
        // carrying its notes, in every operating mode; blocked launches queue.
        let opts = crate::DispatchOpts {
            plan: true,
            queue_if_blocked: true,
            ..Default::default()
        };
        if let Err(error) = crate::dispatch::dispatch(ctx, id, &opts) {
            let hint = format!("{error:#}; retry with `corral dispatch {id} --plan`");
            crate::messages::report(Some(id), "owner plan revision", hint);
        }
    }
    Ok((review, refreshed))
}

/// Reconcile an already completed independent review under explicit automatic policy.
pub(crate) fn accept_auto(ctx: &Ctx, id: &str, plan_id: &str) -> Result<()> {
    let _action = corral_core::work::action(id, OperatingActionKind::AcceptPlan);
    anyhow::ensure!(
        !crate::recovery::plan_recovery_bound(ctx, id, plan_id)?,
        "recovery-bound plan requires explicit owner acceptance"
    );
    let snapshot = ctx.store.head(id)?;
    let selected = snapshot
        .last_plan
        .as_ref()
        .filter(|p| p.id == plan_id)
        .context("stale plan acceptance")?;
    let review = selected
        .current_review
        .as_ref()
        .context("independent review required")?;
    let mut action = crate::operating::identity(
        &snapshot.id,
        selected.run.as_deref().unwrap_or(plan_id),
        OperatingActionKind::AcceptPlan,
        &format!("accept-{}", review.id),
    );
    action.plan = Some(plan_id.into());
    let authorization =
        crate::operating::authorize(ctx, action, crate::operating::Origin::Automatic)?;
    crate::operating::submit_head(
        ctx,
        &authorization,
        |store, task| {
            anyhow::ensure!(
                !crate::recovery::plan_recovery_bound(ctx, id, plan_id)?,
                "recovery-bound plan requires explicit owner acceptance"
            );
            let config = &store.config;
            // The plan's planning run is read by key; the checks read the current plan and
            // its current review, which the head's shell holds.
            let run_id = task
                .head()
                .last_plan
                .as_ref()
                .filter(|p| p.id == plan_id)
                .context("stale plan acceptance")?
                .run
                .clone();
            let run = match run_id.as_deref() {
                Some(id) => task.plan_run_head(id)?,
                None => None,
            };
            if run
                .as_ref()
                .is_some_and(|r| r.state == PlanRunState::Cancelled)
            {
                bail!("cancelled plan run requires explicit replacement dispatch");
            }
            let shell = task.shell_task()?;
            let selected = shell.plans.last().context("stale plan acceptance")?;
            plan::fresh(ctx, &shell, selected)?;
            let digest = plan::policy_digest(config, &shell)?;
            let review = selected
                .reviews
                .last()
                .context("independent review required")?;
            if review.verdict != Some(Verdict::Approve)
                || review.id != authorization.action.attempt.trim_start_matches("accept-")
                || !corral_core::plan_policy::plan_reviewer_authorized(
                    store,
                    config,
                    &shell,
                    review,
                    &plan::expected_reviewer(config, &shell)?,
                )?
                || matches!(review.reviewer.as_str(), "owner" | "pm")
                || review.policy_digest != digest
                || !crate::review_contracts::plan_route_ok(config, &shell, review)
                || plan::owner_decisions(&shell, selected)
            {
                bail!(
                    "automatic plan acceptance requires current independent approval without owner decisions"
                );
            }
            let by = review.reviewer.clone();
            crate::plan_decompose::check_attachment(store, &*task, plan_id)?;
            crate::plan_decompose::check_scopes(store, &*task, plan_id)?;
            task.last_plan_mut()
                .filter(|p| p.id == plan_id)
                .context(
                    "stale plan verdict: plan was replaced; inspect `corral plan show <id> --json`",
                )?
                .approval = Some(PlanApproval {
                by,
                at: corral_core::clock::now(),
                policy_digest: digest,
                authorization: Some(authorization.id.clone()),
            });
            if let Some(run) = &run {
                task.edit_plan_run(&run.id, |r| {
                    r.state = PlanRunState::Approved;
                    Ok(())
                })?;
            }
            task.queued = false;
            if run_id.is_some() && !plan::published_design(&task.shell_task()?) {
                task.status = corral_core::Status::Ready;
            }
            Ok(())
        },
        || Ok(()),
    )?;
    crate::plan_decompose::apply_auto(ctx, id, plan_id)?;
    Ok(())
}
