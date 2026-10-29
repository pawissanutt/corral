//! A refused review keeps its exact attempt without reserving a live process slot.
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::{ActionAuthorization, HerdrRef, PlanRecord, PlanReview, SubmissionDisposition};
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

fn path(ctx: &Ctx, task: &str, plan: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join(format!("plan-review-{task}-{plan}.json"))
}

impl Prepared {
    pub fn save(&self, ctx: &Ctx, task: &str) -> Result<()> {
        let path = path(ctx, task, &self.plan);
        let mut file = tempfile::NamedTempFile::new_in(ctx.store.state_dir())
            .context("staging prepared plan review")?;
        file.write_all(&serde_json::to_vec(self)?)
            .context("writing prepared plan review")?;
        file.persist(path)
            .context("publishing prepared plan review")?;
        Ok(())
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

    /// The retained reviewer pane, when it is still open in this session. A suspended attempt
    /// never started its agent, so a pane closed since then is replaced rather than failing.
    pub fn live_pane(&self, ctx: &Ctx) -> Result<Option<HerdrRef>> {
        let Some(herdr) = &self.review.herdr else {
            return Ok(None);
        };
        ensure!(
            herdr.socket.as_ref() == Some(&ctx.herdr.socket_path()?),
            "prepared reviewer belongs to another session"
        );
        Ok(ctx.herdr.pane_get(&herdr.pane_id)?.map(|_| herdr.clone()))
    }

    pub fn suspend(&self, ctx: &Ctx, task: &str) -> Result<()> {
        self.save(ctx, task)?;
        ctx.store.update(task, |task| {
            let Some(plan) = task.plans.last_mut().filter(|p| p.id == self.plan) else {
                return Ok(());
            };
            // A refusal is not a verdict. Keep the pending attempt in this durable
            // journal and restore it unchanged when it can reserve capacity again.
            if plan
                .reviews
                .last()
                .is_some_and(|r| r.id == self.review.id && r.verdict.is_none())
            {
                plan.reviews.pop();
            }
            Ok(())
        })?;
        Ok(())
    }
}

pub(super) fn load(ctx: &Ctx, task: &str) -> Result<Option<Prepared>> {
    let current = ctx.store.get(task)?;
    let Some(plan) = current.plans.last() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(path(ctx, task, &plan.id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading prepared plan review"),
    };
    let prepared: Prepared =
        serde_json::from_slice(&bytes).context("parsing prepared plan review")?;
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
