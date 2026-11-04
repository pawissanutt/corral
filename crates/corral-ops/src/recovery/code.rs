use super::*;

pub(crate) fn code_review_record(
    ctx: &Ctx,
    task: &Task,
    review: &ReviewRecord,
) -> Result<Option<RecoveryRecord>> {
    let attempt = review.route.as_deref().and_then(|r| r.attempt.as_deref());
    let record = records(ctx,&task.id)?.into_iter().find(|r| matches!(&r.preview.proposed_effect, RecoveryEffect::ReviewAttempt { stage: RecoveryReviewStage::CodeReview, attempt: a, reviewer } if Some(a.as_str()) == attempt && reviewer.profile == review.reviewer));
    if let Some(record) = &record {
        validate_contract(ctx, task, record)?;
        let RecoveryIdentity::Code { candidate, .. } = &record.preview.request.target.identity
        else {
            anyhow::bail!("review recovery candidate missing")
        };
        ensure!(
            review.identity.as_ref() == Some(candidate),
            "recovery callback candidate differs from its one-attempt contract"
        );
    }
    Ok(record)
}

/// Persist a reverse link to the existing landing journal before Git advances.
pub(crate) fn bind_landing(
    ctx: &Ctx,
    record: &RecoveryRecord,
    journal: Option<&str>,
) -> Result<()> {
    let value = (reference(record), &record.preview.proposed_effect, journal);
    corral_core::records::RecordArea::new("recovery-landings")
        .save_intent(&ctx.store, &record.id, &value)
}

/// Join the existing task/landing transaction, not a second task write. A restart after Done
/// but before receipt finalization can prove even KeepBranch without guessing from status.
pub(crate) fn code_acceptance_committed(
    ctx: &Ctx,
    record: &RecoveryRecord,
    sha: &str,
) -> Result<()> {
    let RecoveryEffect::CodeAcceptance { candidate, .. } = &record.preview.proposed_effect else {
        anyhow::bail!("invalid code acceptance effect")
    };
    ensure!(
        sha == candidate.head,
        "recovery landed a different candidate"
    );
    ctx.store.save_record(
        "recovery-code-committed",
        &record.id,
        &(
            reference(record),
            record.preview.proposed_effect.clone(),
            sha,
        ),
    )
}

pub(super) fn code_committed(ctx: &Ctx, record: &RecoveryRecord) -> Result<bool> {
    let saved: Option<(RecoveryReference, RecoveryEffect, String)> =
        ctx.store.record("recovery-code-committed", &record.id)?;
    Ok(saved.is_some_and(|(r,e,sha)| r == reference(record) && e == record.preview.proposed_effect && matches!(e, RecoveryEffect::CodeAcceptance { candidate, .. } if candidate.head == sha)))
}

pub(crate) fn code_acceptance_link(ctx: &Ctx, id: &str, record: &RecoveryRecord) -> Result<()> {
    let RecoveryEffect::CodeAcceptance { candidate, merge } = &record.preview.proposed_effect
    else {
        anyhow::bail!("invalid code acceptance recovery")
    };
    let mut saved = records(ctx, id)?
        .into_iter()
        .find(|r| r.id == record.id)
        .context("code recovery intent disappeared")?;
    let receipt = RecoveryReceipt::CodeAcceptance {
        candidate: candidate.clone(),
    };
    if !saved.links.iter().any(|l| l.receipt == receipt) {
        saved.links.push(RecoveryLink {
            recovery: reference(record),
            receipt,
            acceptance: Some(acceptance(record)),
        });
    }
    if *merge != RecoveryMerge::Keep {
        let receipt = RecoveryReceipt::LandingJournal {
            run: candidate.run.clone(),
            candidate: candidate.clone(),
        };
        if !saved.links.iter().any(|l| l.receipt == receipt) {
            saved.links.push(RecoveryLink {
                recovery: reference(record),
                receipt,
                acceptance: None,
            });
        }
    }
    ctx.store.save_recovery_record(id, &saved)?;
    Ok(())
}

pub(crate) fn check_code_launch(
    ctx: &Ctx,
    task: &Task,
    record: &RecoveryRecord,
    name: &str,
) -> Result<()> {
    validate_contract(ctx, task, record)?;
    ensure!(
        state(record)? == RecoveryState::Applying,
        "recovery permission already consumed"
    );
    ensure!(
        evaluate::task_revision(task)? == record.preview.request.target.task_revision,
        "recovery_stale: task changed before code review"
    );
    let RecoveryEffect::ReviewAttempt {
        stage: RecoveryReviewStage::CodeReview,
        reviewer,
        ..
    } = &record.preview.proposed_effect
    else {
        anyhow::bail!("invalid code review recovery")
    };
    ensure!(reviewer.profile == name, "confirmed reviewer differs");
    Ok(())
}

pub(crate) fn check_resource(
    ctx: &Ctx,
    id: &str,
    record: &RecoveryRecord,
    stage: RecoveryAdmissionStage,
) -> Result<()> {
    ensure!(
        state(record)? == RecoveryState::Applying,
        "resource recovery already consumed"
    );
    let RecoveryAction::ForceAdmission {
        stage: expected,
        pressure_episode,
        thresholds_digest,
    } = &record.preview.request.action
    else {
        anyhow::bail!("not resource authorization")
    };
    ensure!(
        stage == *expected,
        "resource authorization does not apply to this stage"
    );
    let task = ctx.store.get(id)?;
    validate_contract(ctx, &task, record)?;
    ensure!(
        hash(&ctx.store.reopen()?.config.resources)? == *thresholds_digest
            && super::admission::resource_binding(ctx)?.as_ref() == Some(pressure_episode),
        "recovery_stale: pressure episode or thresholds changed"
    );
    Ok(())
}
