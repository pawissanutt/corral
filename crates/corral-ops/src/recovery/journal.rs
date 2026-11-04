use super::*;

pub(crate) fn bind_journal(
    ctx: &Ctx,
    record: &RecoveryRecord,
    authorization: &ActionAuthorization,
) -> Result<()> {
    ensure!(
        authorization.action.task == record.preview.request.target.task,
        "recovery launch journal task mismatch"
    );
    let (kind, attempt) = match &record.preview.proposed_effect {
        RecoveryEffect::ReviewAttempt { stage, attempt, .. } => (
            match stage {
                RecoveryReviewStage::PlanReview => OperatingActionKind::PlanReview,
                RecoveryReviewStage::CodeReview => OperatingActionKind::Review,
            },
            attempt,
        ),
        RecoveryEffect::Admission { stage, attempt } => (
            match stage {
                RecoveryAdmissionStage::Dispatch => OperatingActionKind::Dispatch,
                RecoveryAdmissionStage::PlanDispatch => OperatingActionKind::Plan,
                RecoveryAdmissionStage::Gate => OperatingActionKind::Gate,
                RecoveryAdmissionStage::CodeReview => OperatingActionKind::Review,
                RecoveryAdmissionStage::PlanReview => OperatingActionKind::PlanReview,
            },
            attempt,
        ),
        RecoveryEffect::PlanAcceptance { .. } | RecoveryEffect::CodeAcceptance { .. } => {
            anyhow::bail!("acceptance is not launch authorization")
        }
    };
    ensure!(
        authorization.action.kind == kind,
        "recovery launch stage mismatch"
    );
    let actual = if matches!(
        kind,
        OperatingActionKind::Dispatch | OperatingActionKind::Plan
    ) {
        &authorization.action.run
    } else {
        &authorization.action.attempt
    };
    ensure!(
        actual == attempt,
        "recovery_stale: journal attempt differs from preview"
    );
    if let Some(previous) = ctx
        .store
        .record::<String>("recovery-journals", &record.id)?
    {
        ensure!(
            previous == authorization.id,
            "recovery authorization cannot bind a second launch journal"
        );
    } else {
        ctx.store
            .save_record("recovery-journals", &record.id, &authorization.id)?;
    }
    Ok(())
}

pub(super) fn delivery(
    ctx: &Ctx,
    record: &RecoveryRecord,
) -> Result<Option<SubmissionDisposition>> {
    ctx.store
        .record::<String>("recovery-journals", &record.id)?
        .map(|id| ctx.store.authorized_submission(&id).map(|r| r.disposition))
        .transpose()
}

pub(super) fn append_links(ctx: &Ctx, record: &mut RecoveryRecord) -> Result<()> {
    let receipt = match &record.preview.proposed_effect {
        RecoveryEffect::ReviewAttempt {
            stage: RecoveryReviewStage::PlanReview,
            attempt,
            ..
        } => {
            let RecoveryIdentity::Plan { plan, .. } = &record.preview.request.target.identity
            else {
                anyhow::bail!("plan identity missing")
            };
            Some(RecoveryReceipt::PlanReview {
                plan: plan.clone(),
                attempt: attempt.clone(),
            })
        }
        RecoveryEffect::ReviewAttempt {
            stage: RecoveryReviewStage::CodeReview,
            ..
        } => {
            let RecoveryIdentity::Code { candidate, .. } = &record.preview.request.target.identity
            else {
                anyhow::bail!("candidate identity missing")
            };
            Some(RecoveryReceipt::CodeReview {
                candidate: candidate.clone(),
            })
        }
        RecoveryEffect::PlanAcceptance { .. }
        | RecoveryEffect::CodeAcceptance { .. }
        | RecoveryEffect::Admission { .. } => None,
    };
    let stage = match &record.preview.proposed_effect {
        RecoveryEffect::ReviewAttempt {
            stage: RecoveryReviewStage::PlanReview,
            ..
        } => Some(RecoveryAdmissionStage::PlanReview),
        RecoveryEffect::ReviewAttempt {
            stage: RecoveryReviewStage::CodeReview,
            ..
        } => Some(RecoveryAdmissionStage::CodeReview),
        RecoveryEffect::Admission { stage, .. } => Some(*stage),
        RecoveryEffect::PlanAcceptance { .. } | RecoveryEffect::CodeAcceptance { .. } => None,
    };
    let journal = if let Some(stage) = stage {
        ctx.store
            .record::<String>("recovery-journals", &record.id)?
            .map(|attempt| RecoveryReceipt::LaunchJournal { stage, attempt })
    } else {
        None
    };
    for receipt in receipt.into_iter().chain(journal) {
        if !record.links.iter().any(|l| l.receipt == receipt) {
            record.links.push(RecoveryLink {
                recovery: reference(record),
                receipt,
                acceptance: None,
            });
        }
    }
    Ok(())
}
