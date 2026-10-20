//! Trusted review-context Auto receipts (PRF-08). Under `reviewer = "auto"` a proposer or PM may
//! read eligible profiles' full guidance and pick a reviewer for one exact candidate; corral
//! validates the receipt against that candidate, the contract floor and the existing policy.
use anyhow::{Context, Result, ensure};
use corral_core::{
    AutoProfileSelection, CandidateIdentity, Config, PolicySource, ProfileReviewSubject,
    ProfileRole, ReviewRequirement, ReviewSelectionContext, Reviewer, ReviewerPolicyRecord, Run,
    Task,
};
use serde::Serialize;

use crate::Ctx;

/// What a review-role selector must copy into its receipt.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewSelectionView {
    pub subject: ProfileReviewSubject,
    pub requirement: ReviewRequirement,
    pub policy: ReviewerPolicyRecord,
}

pub(crate) fn policy_record(config: &Config, task: &Task) -> Result<ReviewerPolicyRecord> {
    let policy = config.task_review_policy(task);
    let source = if task.reviewer.is_some() {
        PolicySource::Task
    } else {
        config
            .review_policy_sources(task.project.as_deref(), task.review)
            .reviewer
    };
    Ok(ReviewerPolicyRecord {
        source,
        selector: corral_core::reviewer_selector(&policy.reviewer),
        require_cross_family: policy.cross_family_required(),
        policy_digest: corral_core::reviewer_policy_digest(&policy)?,
    })
}

/// The gate-backed candidate identity a review launch binds, with its contract floor.
pub(crate) fn implementation_view(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
) -> Result<ReviewSelectionView> {
    let attempt = run
        .gate
        .as_ref()
        .and_then(|g| g.identity.as_ref())
        .map(|i| i.attempt.clone())
        .context("review selection needs a gate receipt for the current candidate")?;
    let identity = crate::verification::review_identity(ctx, task, run, &attempt)?;
    let (signals, _) =
        crate::review_contracts::implementation_signals(&ctx.store.config, task, run, None)?;
    Ok(ReviewSelectionView {
        subject: ProfileReviewSubject::Implementation {
            candidate: Box::new(identity),
        },
        requirement: corral_core::review_requirement(&signals),
        policy: policy_record(&ctx.store.config, task)?,
    })
}

/// Save, launch and verdict pass the exact subject; landing freshness passes None so a
/// verified unchanged-patch carry keeps its receipt.
pub(crate) fn check(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    receipt: &AutoProfileSelection,
    requirement: &ReviewRequirement,
    subject: Option<&ProfileReviewSubject>,
) -> Result<()> {
    let config = &ctx.store.config;
    ensure!(
        receipt.role == ProfileRole::ImplementationReview,
        "receipt is not an implementation-review selection"
    );
    ensure!(
        config.task_review_policy(task).reviewer == Reviewer::Auto,
        "review receipts apply only under reviewer = \"auto\"; explicit reviewers are kept"
    );
    receipt.validate(config, task)?;
    let review = receipt
        .review
        .as_ref()
        .context("review receipt lacks its review contract")?;
    if let Some(subject) = subject {
        let vendor = run.vendor.clone().or_else(|| config.run_vendor(run).ok());
        review.validate(
            config,
            &ReviewSelectionContext {
                task,
                subject,
                author_vendor: vendor.as_deref(),
            },
        )?;
    }
    ensure!(
        review.contract.kind == requirement.contract.kind,
        "receipt contract {} differs from the {} floor",
        review.contract.kind.as_str(),
        requirement.contract.kind.as_str()
    );
    ensure!(
        config.review_capability_ok(&receipt.profile, requirement.capability),
        "receipt reviewer '{}' lacks the required review capability",
        receipt.profile
    );
    config.check_profile_eligibility(
        &receipt.profile,
        ProfileRole::ImplementationReview,
        task.project.as_deref(),
        true,
    )
}

pub(crate) fn for_launch(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    identity: &CandidateIdentity,
    requirement: &ReviewRequirement,
) -> Result<(Option<AutoProfileSelection>, Option<String>)> {
    let subject = ProfileReviewSubject::Implementation {
        candidate: Box::new(identity.clone()),
    };
    let receipts = ctx.store.auto_profile_selections()?;
    let Some(receipt) = receipts.iter().rev().find(|r| {
        r.task == task.id
            && r.role == ProfileRole::ImplementationReview
            && r.review.as_ref().is_some_and(|v| v.subject == subject)
    }) else {
        return Ok((None, None));
    };
    Ok(
        match check(ctx, task, run, receipt, requirement, Some(&subject)) {
            Ok(()) => (Some(receipt.clone()), None),
            Err(error) => (
                None,
                Some(format!("review receipt {} ignored: {error:#}", receipt.id)),
            ),
        },
    )
}

/// The exact plan attempt a plan-review receipt must name, with the plan contract floor.
pub(crate) fn plan_view(ctx: &Ctx, task: &Task, attempt: &str) -> Result<ReviewSelectionView> {
    let plan = task
        .plans
        .last()
        .context("submit a plan before selecting its reviewer")?;
    Ok(ReviewSelectionView {
        subject: ProfileReviewSubject::Plan {
            task: task.id.clone(),
            plan: plan.id.clone(),
            spec_revision: corral_core::plan_contract_revision(task)?,
            attempt: attempt.into(),
        },
        requirement: corral_core::review_requirement(&corral_core::ReviewSignals::for_task(
            task,
            corral_core::ReviewSubjectKind::Plan,
        )),
        policy: policy_record(&ctx.store.config, task)?,
    })
}

pub(crate) fn check_plan(
    ctx: &Ctx,
    task: &Task,
    receipt: &AutoProfileSelection,
    view: &ReviewSelectionView,
) -> Result<()> {
    let config = &ctx.store.config;
    ensure!(
        receipt.role == ProfileRole::PlanReview,
        "receipt is not a plan-review selection"
    );
    ensure!(
        config.task_review_policy(task).reviewer == Reviewer::Auto,
        "review receipts apply only under reviewer = \"auto\"; explicit reviewers are kept"
    );
    receipt.validate(config, task)?;
    let review = receipt
        .review
        .as_ref()
        .context("review receipt lacks its review contract")?;
    review.validate(
        config,
        &ReviewSelectionContext {
            task,
            subject: &view.subject,
            author_vendor: None,
        },
    )?;
    ensure!(
        review.contract.kind == view.requirement.contract.kind,
        "receipt contract {} differs from the {} floor",
        review.contract.kind.as_str(),
        view.requirement.contract.kind.as_str()
    );
    ensure!(
        config.review_capability_ok(&receipt.profile, view.requirement.capability),
        "receipt reviewer '{}' lacks the required review capability",
        receipt.profile
    );
    config.check_profile_eligibility(
        &receipt.profile,
        ProfileRole::PlanReview,
        task.project.as_deref(),
        true,
    )?;
    corral_core::plan_policy::independent_reviewer(config, task, &receipt.profile)
}

pub(crate) fn plan_for_launch(
    ctx: &Ctx,
    task: &Task,
    attempt: &str,
) -> Result<(Option<AutoProfileSelection>, Option<String>)> {
    let view = plan_view(ctx, task, attempt)?;
    let receipts = ctx.store.auto_profile_selections()?;
    let Some(receipt) = receipts.iter().rev().find(|r| {
        r.task == task.id
            && r.role == ProfileRole::PlanReview
            && r.review.as_ref().is_some_and(|v| v.subject == view.subject)
    }) else {
        return Ok((None, None));
    };
    Ok(match check_plan(ctx, task, receipt, &view) {
        Ok(()) => (Some(receipt.clone()), None),
        Err(error) => (
            None,
            Some(format!("review receipt {} ignored: {error:#}", receipt.id)),
        ),
    })
}

pub(crate) fn plan_still_valid(ctx: &Ctx, task: &Task, id: &str, attempt: &str) -> Result<bool> {
    let view = plan_view(ctx, task, attempt)?;
    let receipts = ctx.store.auto_profile_selections()?;
    Ok(receipts
        .iter()
        .find(|r| r.id == id)
        .is_some_and(|r| check_plan(ctx, task, r, &view).is_ok()))
}

pub(crate) fn still_valid(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    id: &str,
    requirement: &ReviewRequirement,
    subject: Option<&ProfileReviewSubject>,
) -> Result<bool> {
    let receipts = ctx.store.auto_profile_selections()?;
    Ok(receipts
        .iter()
        .find(|r| r.id == id)
        .is_some_and(|r| check(ctx, task, run, r, requirement, subject).is_ok()))
}
