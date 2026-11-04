use super::*;

/// Observe the protected subject after a known effect, without minting passing receipts or
/// renewing exception authority. Concurrent replacement leaves an explicit diagnostic.
pub(super) fn applied(ctx: &Ctx, record: &RecoveryRecord, detail: &str) -> RecoveryOutcome {
    let mut result = outcome(
        RecoveryState::Applied,
        detail,
        Some(record.preview.proposed_effect.clone()),
    );
    match after(ctx, record) {
        Ok(target) => result.after = Some(target),
        Err(error) => result
            .evidence
            .push(format!("after identity unavailable: {error:#}")),
    }
    result
}

fn after(ctx: &Ctx, record: &RecoveryRecord) -> Result<RecoveryTarget> {
    let task = ctx.store.get(&record.preview.request.target.task)?;
    let mut identity = record.preview.request.target.identity.clone();
    match &mut identity {
        RecoveryIdentity::Plan {
            plan,
            artifact_sha256,
            review_attempt,
        } => {
            let current = task
                .plans
                .last()
                .filter(|p| p.id == *plan)
                .context("plan replaced after effect")?;
            *artifact_sha256 = current.artifact.sha256.clone();
            *review_attempt = current.reviews.last().map(|r| r.id.clone());
        }
        RecoveryIdentity::Code {
            candidate,
            gate_attempt,
            review_attempt,
        } => {
            let run = task
                .current_run()
                .filter(|r| r.id == candidate.run)
                .context("run replaced after effect")?;
            *candidate = crate::verification::identity(ctx, &task, &RunHead::of(run)?, candidate)?;
            *gate_attempt = run
                .gate
                .as_ref()
                .and_then(|g| g.identity.as_ref().map(|i| i.attempt.clone()));
            *review_attempt = run
                .review
                .as_ref()
                .and_then(crate::reviewer::review_callback_token);
        }
        RecoveryIdentity::Admission {
            stage,
            run,
            plan,
            candidate,
            ..
        } => {
            *run = if *stage == RecoveryAdmissionStage::PlanDispatch {
                task.plan_runs.last().map(|r| r.id.clone())
            } else {
                task.current_run().map(|r| r.id.clone())
            };
            *plan = task.plans.last().map(|p| p.id.clone());
            if candidate.is_some() {
                let current = task.current_run().context("run replaced after admission")?;
                let expected = current
                    .gate
                    .as_ref()
                    .and_then(|g| g.identity.as_ref())
                    .or_else(|| current.review.as_ref().and_then(|r| r.identity.as_ref()))
                    .context("admitted candidate identity missing")?;
                *candidate = Some(crate::verification::identity(
                    ctx,
                    &task,
                    &RunHead::of(current)?,
                    expected,
                )?);
            }
        }
    }
    Ok(RecoveryTarget {
        store: ctx.store.root().to_path_buf(),
        task: task.id.clone(),
        task_revision: evaluate::task_revision(&task)?,
        spec_revision: crate::plan::plan_spec_revision(&task)?,
        identity,
    })
}
