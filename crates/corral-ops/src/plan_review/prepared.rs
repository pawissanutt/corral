//! A refused review keeps its exact attempt without reserving a live process slot.
use corral_core::records::RecordArea;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::{
    ActionAuthorization, HerdrRef, PlanRecord, PlanReview, ResolvedProfile, SubmissionDisposition,
    Task,
};
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
    pub fn bind_native(
        &mut self,
        ctx: &Ctx,
        task: &corral_core::TaskHead,
        resolved: &ResolvedProfile,
        herdr: &HerdrRef,
    ) -> Result<()> {
        // T-626: preserve main's native bind/resume and complete prepared-review journal
        // until native mutation/retention storage can be redesigned independently.
        let _known = corral_core::work::followup("T-626", "plan_reviewer_native_binding");
        let target = crate::agent::BindTarget::PlanReviewer {
            task: task.id.clone(),
            plan: self.plan.clone(),
            review: self.review.id.clone(),
        };
        let cwd = ctx
            .store
            .project_path(task.project.as_deref().context("plan project missing")?)?;
        let command = crate::agent::hooks::launch::plan_command(
            ctx,
            &target,
            &herdr.pane_id,
            resolved,
            &cwd,
            &self.prompt,
        )?;
        let receipt = crate::launch_exit::receipt(
            ctx,
            &task.id,
            &format!("{}-{}", self.plan, self.review.id),
        )?;
        self.command = crate::launch_exit::arm(ctx, &command, &receipt)?;
        self.review = ctx
            .store
            .get(&task.id)?
            .plans
            .last()
            .and_then(|p| p.reviews.last())
            .context("plan review missing")?
            .clone();
        self.save(ctx, &task.id)
    }

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
                .is_some_and(|r| r.id == self.review.id && r.verdict.is_none() && !r.cancelled)
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

/// Native commands cannot be constructed until their saved pane has a binding.
pub(super) fn command(
    ctx: &Ctx,
    resolved: &ResolvedProfile,
    task: &Task,
    prompt_file: &Path,
    native: bool,
) -> Result<String> {
    if native {
        crate::agent::hooks::preflight::check(
            ctx,
            corral_core::agent::AgentRole::PlanReviewer,
            resolved,
        )?;
        return Ok(String::new());
    }
    crate::prompt::render_template(
        resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", format!("{} plan review", task.id)),
            ("task", task.id.clone()),
            ("worktree", ctx.store.root().display().to_string()),
        ],
    )
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

/// Read a suspended submission's actual refusal, not just today's broader grant.
pub(crate) fn blocker(
    ctx: &Ctx,
    task: &(impl corral_core::TaskRef + ?Sized),
) -> Result<Option<(jiff::Timestamp, String)>> {
    if task
        .last_plan_head()?
        .is_none_or(|plan| plan.approval.is_some())
    {
        return Ok(None);
    }
    let Some(prepared) = load(ctx, task.id())? else {
        return Ok(None);
    };
    let record = ctx
        .store
        .authorized_submission(&prepared.authorization.id)?;
    Ok(match record.disposition {
        SubmissionDisposition::AuthorizationRefused { reason } => Some((
            prepared.review.at,
            format!("Prepared plan review refused: {reason}"),
        )),
        SubmissionDisposition::Prepared
        | SubmissionDisposition::Submitting
        | SubmissionDisposition::Submitted
        | SubmissionDisposition::Uncertain { .. } => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::{AuthorizedSubmission, Status, Store};
    use serde_json::json;

    #[test]
    fn refused_preparation_is_projected_without_replaying_or_mutating_it() {
        // Given a suspended plan review and its exact authorization journal.
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store.create("Plan", None, Status::Review, "").unwrap();
        let at: jiff::Timestamp = "2026-10-29T01:00:00Z".parse().unwrap();
        let task = store
            .update(&task.id, |task| {
                task.plans.push(serde_json::from_value(json!({
                    "id":"P1", "spec_revision":"spec",
                    "artifact":{"run":"P1","kind":"plan","title":"Plan","path":"plan.md",
                        "source":"/fixture/plan.md","sha256":"sha","bytes":0,"added":at},
                    "document":{"goal":"fixture","non_goals":[],"decisions":[],
                        "affected_paths":[],"contracts":[],"dependencies":[],
                        "acceptance":[],"verification":[],"owner_decisions":[]}
                }))?);
                Ok(())
            })
            .unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(store);
        let prepared = Prepared {
            plan: "P1".into(),
            artifact_sha256: "sha".into(),
            spec_revision: "spec".into(),
            previous_reviews: 0,
            review: serde_json::from_value(json!({
                "id":"PR1","reviewer":"fixture","policy_digest":"policy","at":at
            }))
            .unwrap(),
            authorization: serde_json::from_value(json!({
                "id":"a".repeat(64),"workspace":root.path(),
                "member":{"project":"fixture","repository":root.path(),
                    "repository_identity":"repo","contract_revision":"contract"},
                "policy_revision":"old-policy",
                "action":{"task":task.id,"run":"P1","attempt":"PR1",
                    "kind":"plan_review","candidate":null,"plan":"P1","round":null},
                "origin":{"kind":"automatic","authorization":"old-grant","revision":1}
            }))
            .unwrap(),
            prompt: root.path().join("prompt.md"),
            command: "never executed".into(),
        };
        prepared.save(&ctx, &task.id).unwrap();
        ctx.store
            .fixture_save_submission(&AuthorizedSubmission {
                version: 1,
                authorization: prepared.authorization.clone(),
                disposition: SubmissionDisposition::Prepared,
                authorization_history: vec![],
            })
            .unwrap();
        for (disposition, stopped) in [
            (
                SubmissionDisposition::AuthorizationRefused {
                    reason: "policy changed".into(),
                },
                true,
            ),
            (SubmissionDisposition::Prepared, false),
            (SubmissionDisposition::Submitted, false),
        ] {
            let record = AuthorizedSubmission {
                version: 1,
                authorization: prepared.authorization.clone(),
                disposition,
                authorization_history: vec![],
            };
            if record.disposition == SubmissionDisposition::Submitted {
                let mut submitting = record.clone();
                submitting.disposition = SubmissionDisposition::Submitting;
                ctx.store.fixture_save_submission(&submitting).unwrap();
            }
            ctx.store.fixture_save_submission(&record).unwrap();
            // When the read-only blocker projection reads the retained preparation.
            let result = blocker(&ctx, &task).unwrap();
            // Then only a refused, unsubmitted action is blocked, anchored to this attempt.
            assert_eq!(result.is_some(), stopped);
            if let Some((since, _)) = result {
                assert_eq!(since, at);
            }
            assert_eq!(
                ctx.store
                    .authorized_submission(&prepared.authorization.id)
                    .unwrap(),
                record
            );
            assert_eq!(ctx.store.get(&task.id).unwrap(), task);
        }
    }
}
