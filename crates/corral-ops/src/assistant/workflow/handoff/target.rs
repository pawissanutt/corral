//! Existing-target identity is separate from the immutable source package. It binds
//! only current semantic plan/producer/findings/approval and code, never full history.
use crate::Ctx;
use anyhow::{Context, Result, ensure};
use corral_core::TaskRef;

pub(super) fn pin<T: TaskRef>(ctx: &Ctx, task: &T) -> Result<String> {
    let plan = task.last_plan_head()?;
    let producer = match plan.as_ref().and_then(|p| p.run.as_deref()) {
        Some(id) => Some(
            task.plan_run_head(id)?
                .context("handoff_target_changed: plan producer missing")?,
        ),
        None => None, // actual legacy/human provenance stays unnamed, never relabelled
    };
    let plan = plan.as_ref().map(|p| {
        (
            &p.id,
            &p.artifact.sha256,
            &p.spec_revision,
            &p.document,
            &p.run,
            p.facts.children,
            p.current_review.as_ref().map(|r| {
                (
                    &r.id,
                    &r.reviewer,
                    r.verdict,
                    &r.notes,
                    &r.policy_digest,
                    &r.route,
                    r.cancelled,
                )
            }),
            &p.approval,
        )
    });
    let producer = producer.as_ref().map(|p| {
        (
            &p.id,
            &p.profile,
            &p.harness,
            &p.model,
            &p.vendor,
            &p.effort,
            &p.spec_revision,
            &p.execution,
        )
    });
    let run = task.current_run_head()?;
    let code = if task
        .cowork()
        .as_ref()
        .is_some_and(|c| c.review == corral_core::CoworkReview::TransferV1)
    {
        super::code_pin::current(ctx, task)?
    } else if let Some(run) = &run {
        Some(super::code_pin::of_state(
            Some(run.id.clone()),
            super::git::scan(&run.worktree, &run.base_sha, &run.branch)?.state,
        )?)
    } else {
        None
    };
    // RunHead has current gate/review/completion only, not previous runs/reviews.
    Ok(crate::verification::digest(&serde_json::to_vec(&(
        plan, producer, run, code,
    ))?))
}

pub(super) fn require(
    ctx: &Ctx,
    task: &corral_core::Task,
    target: &super::transfer::Target,
) -> Result<()> {
    ensure!(
        task.id == target.task
            && task.contract_revision()? == target.contract_revision
            && task.current_run().map(|r| &r.id) == target.run.as_ref(),
        "handoff_stale_choice: target task/spec/run changed; refresh"
    );
    ensure!(
        target
            .state
            .as_ref()
            .is_some_and(|expected| expected.len() == 64)
            && target.state.as_deref() == Some(pin(ctx, task)?.as_str()),
        "handoff_target_changed: plan, actual producer, findings, approval or code changed; refresh the target choice"
    );
    Ok(())
}
