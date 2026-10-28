//! A refused review keeps its exact attempt without reserving a live process slot.
use std::path::PathBuf;

use anyhow::{Result, ensure};
use corral_core::records::RecordArea;
use corral_core::{ActionAuthorization, PlanRecord, PlanReview, SubmissionDisposition};
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Prepared {
    pub plan: String,
    pub artifact_sha256: String,
    pub spec_revision: String,
    pub previous_reviews: usize,
    pub review: PlanReview,
    pub authorization: ActionAuthorization,
    pub prompt: PathBuf,
    pub command: String,
}

const PREPARED: RecordArea<Prepared> = RecordArea::new("plan-review-prepared");

impl Prepared {
    pub fn save(&self, ctx: &Ctx, task: &str) -> Result<()> {
        PREPARED.save(&ctx.store, &format!("{task}-{}", self.plan), self)
    }

    pub fn check(&self, plan: &PlanRecord) -> Result<()> {
        ensure!(
            self.plan == plan.id
                && self.artifact_sha256 == plan.artifact.sha256
                && self.spec_revision == plan.spec_revision,
            "prepared review plan identity changed"
        );
        Ok(())
    }

    pub fn suspend(&self, ctx: &Ctx, task: &str) -> Result<()> {
        ctx.store.update_head(task, |task| {
            self.save(ctx, &task.id)?;
            let Some(plan) = task.head().last_plan.as_ref().filter(|p| p.id == self.plan) else {
                return Ok(());
            };
            // A refusal is not a verdict. Keep the pending attempt in this durable
            // journal and restore it unchanged when it can reserve capacity again.
            if plan
                .current_review
                .as_ref()
                .is_some_and(|r| r.id == self.review.id && r.verdict.is_none())
            {
                task.pop_current_plan_review(&self.plan)?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

/// Whether a refused launch retains review attempt `review` of plan `plan` for a later resume.
pub(crate) fn retains(ctx: &Ctx, task: &str, plan: &str, review: &str) -> Result<bool> {
    Ok(PREPARED
        .load(&ctx.store, &format!("{task}-{plan}"))?
        .is_some_and(|prepared| prepared.review.id == review))
}

pub(super) fn load(ctx: &Ctx, task: &str) -> Result<Option<Prepared>> {
    let current = ctx.store.head(task)?;
    let Some(plan) = current.last_plan.as_ref() else {
        return Ok(None);
    };
    load_for(ctx, task, &plan.id)
}

/// Whether a prepared review of plan `plan`, the task's last, awaits its launch, read by key.
pub(crate) fn has_prepared(ctx: &Ctx, task: &str, plan: &str) -> Result<bool> {
    Ok(load_for(ctx, task, plan)?.is_some())
}

/// The prepared review of plan `plan`, the task's last, still awaiting its launch.
fn load_for(ctx: &Ctx, task: &str, plan: &str) -> Result<Option<Prepared>> {
    let Some(prepared) = PREPARED.load(&ctx.store, &format!("{task}-{plan}"))? else {
        return Ok(None);
    };
    match ctx
        .store
        .authorized_submission(&prepared.authorization.id)?
        .disposition
    {
        SubmissionDisposition::Prepared | SubmissionDisposition::AuthorizationRefused { .. } => {
            Ok(Some(prepared))
        }
        SubmissionDisposition::Submitting
        | SubmissionDisposition::Submitted
        | SubmissionDisposition::Uncertain { .. } => Ok(None),
    }
}
