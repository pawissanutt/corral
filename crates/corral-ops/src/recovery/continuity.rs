use super::*;

/// Validate the unmodified policy and exact plan bytes throughout the exception chain.
pub(crate) fn validate_contract(ctx: &Ctx, task: &Task, record: &RecoveryRecord) -> Result<()> {
    ensure!(
        record.preview.request.target.store == ctx.store.root()
            && record.preview.request.target.task == task.id,
        "recovery target mismatch"
    );
    ensure!(
        !matches!(state(record)?, RecoveryState::Failed),
        "recovery operation failed"
    );
    let current = policy::capture(
        &ctx.store.reopen()?.config,
        task,
        &record.preview.request.action,
    )?;
    ensure!(
        current == record.preview.policy,
        "recovery_stale: raw policy, sources or reviewer definitions changed"
    );
    match &record.preview.request.target.identity {
        RecoveryIdentity::Plan {
            plan,
            artifact_sha256,
            ..
        } => {
            let selected = task
                .plans
                .last()
                .filter(|p| p.id == *plan && p.artifact.sha256 == *artifact_sha256)
                .context("recovery_stale: plan replaced")?;
            crate::plan::fresh(ctx, task, selected)?;
        }
        RecoveryIdentity::Code { candidate, .. } => {
            let run = task
                .current_run()
                .filter(|r| r.id == candidate.run)
                .context("recovery_stale: run replaced")?;
            ensure!(
                crate::verification::identity(ctx, task, &RunHead::of(run)?, candidate)?
                    == *candidate,
                "recovery_stale: candidate or base changed"
            );
        }
        RecoveryIdentity::Admission {
            candidate,
            run,
            plan,
            ..
        } => {
            if let Some(candidate) = candidate {
                let current = task
                    .current_run()
                    .filter(|r| r.id == candidate.run)
                    .context("admission recovery run replaced")?;
                ensure!(
                    crate::verification::identity(ctx, task, &RunHead::of(current)?, candidate)?
                        == *candidate,
                    "recovery_stale: admission candidate or base changed"
                );
            } else if let Some(run) = run {
                ensure!(
                    task.current_run().is_some_and(|r| &r.id == run),
                    "admission recovery run changed"
                );
            }
            if let Some(plan) = plan {
                let selected = task
                    .plans
                    .last()
                    .filter(|p| &p.id == plan)
                    .context("admission recovery plan replaced")?;
                crate::plan::fresh(ctx, task, selected)?;
            }
            ensure!(
                crate::plan::plan_spec_revision(task)?
                    == record.preview.request.target.spec_revision,
                "recovery_stale: task specification changed"
            );
        }
    }
    ensure!(
        crate::session_scope::owns_task(ctx, task)?,
        "recovery session ownership changed"
    );
    Ok(())
}

pub(crate) fn plan_review_record(
    ctx: &Ctx,
    task: &Task,
    plan: &str,
    attempt: &str,
) -> Result<Option<RecoveryRecord>> {
    let record = records(ctx, &task.id)?.into_iter().filter(|r| !matches!(state(r), Ok(RecoveryState::Failed))).find(|r| matches!(&r.preview.proposed_effect, RecoveryEffect::ReviewAttempt { stage: RecoveryReviewStage::PlanReview, attempt: a, .. } if a == attempt) && matches!(&r.preview.request.target.identity, RecoveryIdentity::Plan { plan: p, .. } if p == plan));
    if let Some(record) = &record {
        validate_contract(ctx, task, record)?;
    }
    Ok(record)
}

pub(crate) fn plan_review_valid(ctx: &Ctx, task: &Task, plan: &PlanRecord) -> Result<()> {
    let review = plan.reviews.last().context("independent review missing")?;
    let config = crate::plan::config(ctx)?;
    ensure!(
        review.reviewer != "owner" && review.verdict == Some(Verdict::Approve) && !review.cancelled,
        "independent approve missing"
    );
    ensure!(
        crate::review_contracts::plan_route_ok(&config, task, review),
        "review contract or capability changed"
    );
    if let Some(record) = plan_review_record(ctx, task, &plan.id, &review.id)? {
        let RecoveryEffect::ReviewAttempt { reviewer, .. } = &record.preview.proposed_effect else {
            anyhow::bail!("invalid recovered review effect")
        };
        ensure!(
            review.reviewer == reviewer.profile
                && review.policy_digest == record.preview.contract_digest
                && review.herdr.is_some(),
            "recovery review attempt mismatch"
        );
    } else {
        ensure!(
            review.policy_digest == crate::plan::policy_digest(&config, task)?
                && corral_core::plan_policy::plan_reviewer_authorized(
                    &ctx.store,
                    &config,
                    task,
                    review,
                    &crate::plan::expected_reviewer(&config, task)?
                )?,
            "independent reviewer is unauthorized or stale"
        );
    }
    Ok(())
}

pub(crate) fn check_plan_launch(
    ctx: &Ctx,
    task: &Task,
    record: &RecoveryRecord,
    name: &str,
) -> Result<()> {
    validate_contract(ctx, task, record)?;
    ensure!(
        state(record)? == RecoveryState::Applying,
        "recovery launch permission was already consumed"
    );
    let RecoveryEffect::ReviewAttempt {
        stage: RecoveryReviewStage::PlanReview,
        attempt,
        reviewer,
    } = &record.preview.proposed_effect
    else {
        anyhow::bail!("not a plan-review recovery")
    };
    ensure!(
        reviewer.profile == name,
        "recovery selected reviewer mismatch"
    );
    let plan = task.plans.last().context("plan missing")?;
    ensure!(
        format!("PR{}", plan.reviews.len() + 1) == *attempt,
        "recovery review attempt superseded"
    );
    ensure!(
        evaluate::task_revision(task)? == record.preview.request.target.task_revision,
        "recovery_stale: task changed before launch"
    );
    Ok(())
}

pub(crate) fn check_reviewer(record: &RecoveryRecord, resolved: &ResolvedProfile) -> Result<()> {
    let RecoveryEffect::ReviewAttempt { reviewer, .. } = &record.preview.proposed_effect else {
        anyhow::bail!("not a review effect")
    };
    ensure!(
        resolved.profile == reviewer.profile
            && resolved.model.as_ref() == Some(&reviewer.model)
            && resolved.vendor.as_ref() == Some(&reviewer.vendor),
        "recovery_stale: actual reviewer differs from confirmed model"
    );
    Ok(())
}

pub(crate) fn plan_recovery_bound(ctx: &Ctx, id: &str, plan: &str) -> Result<bool> {
    Ok(records(ctx,id)?.iter().any(|r| matches!(&r.preview.request.target.identity, RecoveryIdentity::Plan { plan: p, .. } if p == plan) && !matches!(state(r), Ok(RecoveryState::Failed))))
}

/// A recovered acceptance is checked independently; never manufacture ordinary passing data.
pub(crate) fn recovered_approval(ctx: &Ctx, task: &Task, plan: &PlanRecord) -> Result<bool> {
    let Some(approval) = &plan.approval else {
        return Ok(false);
    };
    let record = records(ctx, &task.id)?.into_iter().find(|r| r.links.iter().any(|l| matches!(&l.receipt, RecoveryReceipt::PlanApproval { plan: p, at } if p == &plan.id && *at == approval.at)));
    let Some(record) = record else {
        return Ok(false);
    };
    validate_contract(ctx, task, &record)?;
    ensure!(
        approval.by == "owner",
        "recovered plan requires explicit owner acceptance"
    );
    let owner = plan
        .reviews
        .last()
        .filter(|r| r.reviewer == "owner" && r.verdict == Some(Verdict::Approve))
        .context("owner acceptance missing")?;
    ensure!(
        crate::plan::decisions_resolved(task, plan, owner.notes.as_deref().unwrap_or_default()),
        "owner decisions remain"
    );
    if matches!(
        record.preview.request.action,
        RecoveryAction::UseReviewer { .. }
            | RecoveryAction::AcceptPlan {
                waive_independent_review: false,
                ..
            }
    ) {
        let mut independent = plan.clone();
        independent.reviews.pop();
        plan_review_valid(ctx, task, &independent)?;
    } else {
        ensure!(
            matches!(
                record.preview.request.action,
                RecoveryAction::AcceptPlan { .. }
            ),
            "invalid plan acceptance recovery"
        );
    }
    ensure!(
        task.plan.as_deref() == Some(&format!("{}@{}", task.id, plan.id)),
        "plan acceptance pending publication"
    );
    Ok(true)
}

pub(crate) fn acceptance_link(
    ctx: &Ctx,
    task: &str,
    plan: &str,
    at: jiff::Timestamp,
    mut record: RecoveryRecord,
) -> Result<()> {
    let receipt = RecoveryReceipt::PlanApproval {
        plan: plan.into(),
        at,
    };
    if !record.links.iter().any(|l| l.receipt == receipt) {
        record.links.push(RecoveryLink {
            recovery: reference(&record),
            receipt,
            acceptance: Some(acceptance(&record)),
        });
        ctx.store.save_recovery_record(task, &record)?;
    }
    Ok(())
}
