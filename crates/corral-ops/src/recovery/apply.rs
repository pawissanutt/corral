use super::*;

/// Apply only an exact preview and an opaque confirmation from the trusted local ceremony.
/// Identical retries return durable evidence without replaying an external command.
pub fn recover_apply(
    ctx: &Ctx,
    preview: &RecoveryPreview,
    input: &RecoveryInput,
    confirmation: Option<OwnerConfirmation>,
) -> Result<RecoveryRecord> {
    let id = &preview.request.target.task;
    let _lock = ctx
        .store
        .try_lock(&format!("recovery-{id}"))?
        .context("recovery operation already in progress")?;
    let operation = format!("REC-{}", preview.digest);
    if let Some(record) = records(ctx, id)?.into_iter().find(|r| r.id == operation) {
        ensure!(
            record.preview == *preview
                && record.preview.request.action == input.action
                && record.preview.request.exceptions == input.exceptions
                && hash(&(
                    &preview.contract_digest,
                    &preview.blockers,
                    &preview.remaining_blockers,
                    &input.notes
                ))? == preview.digest,
            "recovery request changed; obtain a new preview and confirmation"
        );
        project_event(ctx, id, &record)?;
        return Ok(record);
    }
    let confirmation =
        confirmation.context("recovery requires trusted local owner confirmation")?;
    let RecoveryAttestation::LocalOwnerAttestation { interaction, .. } =
        &confirmation.authorization.attestation;
    authority::require_owner_for(ctx, *interaction)?;
    ensure!(
        confirmation.preview == *preview,
        "owner confirmation does not bind this exact preview"
    );
    current_preview(ctx, preview, input)?;
    let mut record = RecoveryRecord {
        version: RECOVERY_VERSION,
        id: operation,
        preview: preview.clone(),
        authorization: confirmation.authorization,
        outcomes: vec![outcome(
            RecoveryState::Authorized,
            "Exact owner-confirmed intent; no effect submitted",
            None,
        )],
        links: vec![],
    };
    ctx.store.save_recovery_record(id, &record)?;
    record.outcomes.push(outcome(
        RecoveryState::Applying,
        "Entering the existing guarded operation",
        None,
    ));
    ctx.store.save_recovery_record(id, &record)?;
    // Once Applying is durable, any process loss is uncertain: no retry submits again.
    let result = execute(ctx, input, &record);
    match result {
        Ok(()) => record.outcomes.push(super::snapshot::applied(
            ctx,
            &record,
            "Guarded operation completed",
        )),
        Err(error) => {
            // Only the executor's typed pre-submission refusal proves no command was sent.
            let state = if crate::operating::refused(&error) {
                RecoveryState::Failed
            } else {
                RecoveryState::Unknown
            };
            record.outcomes.push(outcome(
                state,
                format!("Operation requires journal reconciliation: {error:#}"),
                None,
            ));
        }
    }
    // Callbacks/acceptance may already have appended links while the command was completing.
    let saved = records(ctx, id)?
        .into_iter()
        .find(|r| r.id == record.id)
        .context("recovery intent disappeared")?;
    record.links = saved.links;
    if state(&record)? == RecoveryState::Applied {
        super::journal::append_links(ctx, &mut record)?;
    }
    ctx.store.save_recovery_record(id, &record)?;
    project_event(ctx, id, &record)?;
    Ok(record)
}

fn execute(ctx: &Ctx, input: &RecoveryInput, record: &RecoveryRecord) -> Result<()> {
    let id = &record.preview.request.target.task;
    match &input.action {
        RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::PlanReview,
            reviewer,
        } => {
            crate::plan_review::review_recovery(ctx, id, &reviewer.profile, record)?;
        }
        RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::CodeReview,
            reviewer,
        } => {
            crate::reviewer::review_recovery(ctx, id, &reviewer.profile, record)?;
        }
        RecoveryAction::AcceptPlan { .. } => {
            accept_plan(ctx, input, record)?;
        }
        RecoveryAction::AcceptCode { merge } => {
            let strategy = match merge {
                RecoveryMerge::Keep => crate::ApproveStrategy::KeepBranch,
                RecoveryMerge::Rebase => crate::ApproveStrategy::Rebase,
                RecoveryMerge::Ff => crate::ApproveStrategy::FastForward,
            };
            crate::review::approve_recovery(ctx, id, strategy, record)?;
        }
        RecoveryAction::ForceAdmission { stage, .. } => match stage {
            RecoveryAdmissionStage::Dispatch | RecoveryAdmissionStage::PlanDispatch => {
                super::admission::with_admission(record, || {
                    crate::dispatch(
                        ctx,
                        id,
                        &crate::DispatchOpts {
                            plan: *stage == RecoveryAdmissionStage::PlanDispatch,
                            force_resources: true,
                            ..Default::default()
                        },
                    )
                })?;
            }
            RecoveryAdmissionStage::Gate => {
                super::admission::with_admission(record, || {
                    crate::gate_start_with_force(ctx, id, true)
                })?;
            }
            RecoveryAdmissionStage::PlanReview => {
                super::admission::with_admission(record, || {
                    crate::plan_review::review_forced(ctx, id, record)
                })?;
            }
            RecoveryAdmissionStage::CodeReview => {
                super::admission::with_admission(record, || {
                    crate::reviewer::review_forced(ctx, id, record)
                })?;
            }
        },
    }
    Ok(())
}

fn accept_plan(ctx: &Ctx, input: &RecoveryInput, record: &RecoveryRecord) -> Result<()> {
    let id = &record.preview.request.target.task;
    let RecoveryEffect::PlanAcceptance { plan, .. } = &record.preview.proposed_effect else {
        anyhow::bail!("invalid acceptance effect")
    };
    let (saved, ()) = ctx.store.dispatch_update(id, |store, task| {
        let mut local = ctx.clone();
        local.store = store.clone();
        current_preview(&local, &record.preview, input)?;
        let at = corral_core::clock::now();
        let p = task
            .plans
            .last_mut()
            .filter(|p| p.id == *plan)
            .context("plan replaced")?;
        crate::plan_review::terminalize_pending(p, "Superseded by explicit owner recovery.");
        p.reviews.push(PlanReview {
            context: None,
            id: format!("PR{}", p.reviews.len() + 1),
            reviewer: "owner".into(),
            policy_digest: record.preview.contract_digest.clone(),
            verdict: Some(Verdict::Approve),
            notes: Some(input.notes.clone()),
            at,
            herdr: None,
            pending_reply: None,
            route: None,
            cancelled: false,
            agent_host: None,
            delivery: Default::default(),
            inbox: Default::default(),
        });
        p.approval = Some(PlanApproval {
            by: "owner".into(),
            at,
            policy_digest: record.preview.contract_digest.clone(),
            authorization: None,
        });
        let planner = p.run.clone();
        if let Some(run) = task
            .plan_runs
            .iter_mut()
            .find(|r| Some(&r.id) == planner.as_ref() && r.state != PlanRunState::Cancelled)
        {
            run.state = PlanRunState::Approved;
        }
        task.dispatch_hold = true;
        task.queued = false;
        task.dispatch_plan = None;
        task.status = Status::NeedsMe;
        // The saved owner decision is authoritative even if its receipt projection is interrupted.
        Ok(())
    })?;
    let at = saved
        .plans
        .last()
        .and_then(|p| p.approval.as_ref())
        .context("acceptance missing")?
        .at;
    acceptance_link(ctx, id, plan, at, record.clone())?;
    crate::plan_decompose::apply(ctx, id, plan)?;
    Ok(())
}

/// Reconcile durable observations only. This API never launches or refreshes authority.
pub fn recover_reconcile(ctx: &Ctx, id: &str, operation: &str) -> Result<RecoveryRecord> {
    let _lock = ctx
        .store
        .try_lock(&format!("recovery-{id}"))?
        .context("recovery operation already in progress")?;
    let mut record = records(ctx, id)?
        .into_iter()
        .find(|r| r.id == operation)
        .context("unknown recovery operation")?;
    let task = ctx.store.get(id)?;
    if state(&record)? == RecoveryState::Applied {
        repair_plan_acceptance(ctx, &task, &record)?;
        record = records(ctx, id)?
            .into_iter()
            .find(|r| r.id == operation)
            .context("recovery disappeared")?;
        project_event(ctx, id, &record)?;
        return Ok(record);
    }
    if state(&record)? == RecoveryState::Failed {
        project_event(ctx, id, &record)?;
        return Ok(record);
    }
    let code_acceptance = matches!(
        record.preview.proposed_effect,
        RecoveryEffect::CodeAcceptance { .. }
    );
    let mut uncertain = None;
    let landed = if code_acceptance {
        match super::landing::reconcile(ctx, &record) {
            Ok(observed) => observed,
            Err(error) => {
                uncertain = Some(format!(
                    "Landing proof unavailable; no command replayed: {error:#}"
                ));
                false
            }
        }
    } else {
        false
    };
    if !code_acceptance {
        validate_contract(ctx, &task, &record)?;
    }
    let observed = match &record.preview.proposed_effect {
        RecoveryEffect::ReviewAttempt {
            stage: RecoveryReviewStage::PlanReview,
            attempt,
            reviewer,
        } => task
            .plans
            .last()
            .and_then(|p| p.reviews.iter().find(|r| r.id == *attempt))
            .is_some_and(|r| {
                r.reviewer == reviewer.profile
                    && r.policy_digest == record.preview.contract_digest
                    && (r.verdict.is_some() || (r.herdr.is_some() && r.pending_reply.is_none()))
            }),
        RecoveryEffect::PlanAcceptance { plan, .. } => {
            if let Some(approval) = task
                .plans
                .last()
                .filter(|p| p.id == *plan)
                .and_then(|p| p.approval.as_ref())
                .filter(|a| a.policy_digest == record.preview.contract_digest && a.by == "owner")
            {
                acceptance_link(ctx, id, plan, approval.at, record.clone())?;
                crate::plan_decompose::apply(ctx, id, plan)?;
                true
            } else {
                false
            }
        }
        RecoveryEffect::CodeAcceptance { .. } => {
            if landed {
                code_acceptance_link(ctx, id, &record)?;
            }
            landed
        }
        RecoveryEffect::ReviewAttempt {
            stage: RecoveryReviewStage::CodeReview,
            ..
        }
        | RecoveryEffect::Admission { .. } => matches!(
            super::journal::delivery(ctx, &record)?,
            Some(SubmissionDisposition::Submitted)
        ),
    };
    record = records(ctx, id)?
        .into_iter()
        .find(|r| r.id == operation)
        .context("recovery disappeared")?;
    if observed {
        super::journal::append_links(ctx, &mut record)?;
        record.outcomes.push(super::snapshot::applied(
            ctx,
            &record,
            "Existing canonical receipt reconciled; no command replayed",
        ));
    } else if state(&record)? != RecoveryState::Unknown {
        record.outcomes.push(outcome(
            RecoveryState::Unknown,
            uncertain.unwrap_or_else(|| {
                "No definitive receipt; external delivery must not be replayed".into()
            }),
            None,
        ));
    }
    ctx.store.save_recovery_record(id, &record)?;
    project_event(ctx, id, &record)?;
    Ok(record)
}

fn repair_plan_acceptance(ctx: &Ctx, task: &Task, record: &RecoveryRecord) -> Result<()> {
    let Some(plan) = task.plans.last() else {
        return Ok(());
    };
    let Some(approval) = plan
        .approval
        .as_ref()
        .filter(|a| a.by == "owner" && a.policy_digest == record.preview.contract_digest)
    else {
        return Ok(());
    };
    validate_contract(ctx, task, record)?;
    ensure!(
        plan.reviews.last().is_some_and(|r| r.reviewer == "owner"
            && r.verdict == Some(Verdict::Approve)
            && crate::plan::decisions_resolved(task, plan, r.notes.as_deref().unwrap_or_default())),
        "owner acceptance decisions are incomplete"
    );
    if matches!(
        record.preview.request.action,
        RecoveryAction::UseReviewer { .. }
    ) {
        let mut independent = plan.clone();
        independent.reviews.pop();
        plan_review_valid(ctx, task, &independent)?;
    }
    acceptance_link(ctx, &task.id, &plan.id, approval.at, record.clone())?;
    crate::plan_decompose::apply(ctx, &task.id, &plan.id)?;
    Ok(())
}

fn project_event(ctx: &Ctx, id: &str, record: &RecoveryRecord) -> Result<()> {
    // Event and projection marker commit together; a restart cannot duplicate the event.
    let key = format!(
        "{}-{}-{}",
        record.id,
        record.outcomes.len(),
        record.links.len()
    );
    ctx.store.records_transaction(|| {
        if ctx.store.record::<bool>("recovery-events", &key)?.is_none() {
            ctx.store.log_event(Some(id), "recovery_outcome", serde_json::json!({"operation":record.id,"state":state(record)?,"recovery":reference(record),"reason":record.authorization.reason,"checks":record.preview.checks}))?;
            ctx.store.save_record("recovery-events", &key, &true)?;
        }
        Ok(())
    })?;
    // A pending code review's packet binds its artifact inputs. The canonical recovery
    // record and event are already durable; publishing another artifact now would stale
    // the genuine callback. Acceptance/readback can materialize the receipt after verdict.
    if ctx
        .store
        .head(id)?
        .current_run
        .as_deref()
        .and_then(|r| r.review.as_ref())
        .is_some_and(|r| r.verdict.is_none())
    {
        return Ok(());
    }
    let temp = tempfile::tempdir().context("creating recovery receipt staging")?;
    let path = temp.path().join("owner-recovery-receipt.json");
    std::fs::write(&path, serde_json::to_vec_pretty(record)?)
        .context("writing recovery provenance receipt")?;
    crate::add_artifact(
        ctx,
        id,
        &path,
        Some(ArtifactKind::Evidence),
        Some("Owner recovery receipt (not ordinary policy success)"),
    )?;
    Ok(())
}
