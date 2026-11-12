//! External delivery is an owner-attested exception, not a native acceptance or Git executor.
use super::*;

pub(super) fn evaluate(
    ctx: &Ctx,
    task: &Task,
    delivery: &ExternalDelivery,
    evaluation: &mut evaluate::Evaluation,
) -> Result<(RecoveryIdentity, RecoveryEffect)> {
    let destination = destination(ctx, task, delivery)?;
    let run = task.current_run().context("external code run missing")?;
    let repair = ctx.store.repair_ledger(&task.id)?;
    evaluation.observation(
        RecoveryCheck::NativeCandidate,
        RecoveryObservation::Unverifiable { reason: "External delivery replaces native candidate acceptance; native freshness is not asserted".into() },
        false,
        vec![serde_json::to_string(&serde_json::json!({
            "run":run.id, "branch":run.branch, "worktree":run.worktree,
            "pending_reply":run.pending_reply, "blocked_excerpt":run.blocked_excerpt,
            "stuck":run.stuck, "obligations":run.obligations,
            "infra_wait":task.infra_wait, "park":task.park, "attention":task.attention,
            "repair_ledger_digest":repair.as_ref().map(RepairLedger::digest).transpose()?,
            "unresolved_findings":repair.as_ref().map(RepairLedger::unresolved).transpose()?
        }))?],
        true,
    );
    let policy = ctx.store.config.task_review_policy(task);
    if !policy.gate.is_empty() {
        evaluation.observation(
            RecoveryCheck::GateOutcome,
            run.gate.as_ref().map_or(RecoveryObservation::Missing, |g| {
                RecoveryObservation::Gate { result: g.result }
            }),
            false,
            vec!["Native gate does not verify the external commit".into()],
            true,
        );
    }
    if crate::review::review_needed(&policy, task) {
        evaluation.observation(
            RecoveryCheck::ReviewOutcome,
            run.review
                .as_ref()
                .map_or(RecoveryObservation::Missing, |r| {
                    RecoveryObservation::Review { verdict: r.verdict }
                }),
            false,
            vec!["Native review does not accept the external commit".into()],
            true,
        );
    }
    if task.completion.is_some() {
        evaluation.observation(
            RecoveryCheck::RequiredEvidence,
            RecoveryObservation::Unverifiable {
                reason: "External evidence replaces native criterion receipt validation".into(),
            },
            false,
            vec![serde_json::to_string(&task.completion)?],
            true,
        );
    }
    Ok((
        RecoveryIdentity::External {
            run: run.id.clone(),
            destination: destination.clone(),
        },
        RecoveryEffect::ExternalCompletion {
            run: run.id.clone(),
            delivery: delivery.clone(),
            destination,
        },
    ))
}

pub(super) fn destination(
    ctx: &Ctx,
    task: &Task,
    delivery: &ExternalDelivery,
) -> Result<ExternalDestination> {
    let run = task
        .current_run()
        .context("external completion requires an existing code run")?;
    ensure!(
        !task.plan_acceptance_required(),
        "Plan completion requires aggregate acceptance"
    );
    crate::operating::require_completion(task)?;
    crate::assistant::workflow::handoff::admission::require(
        &ctx.store,
        task,
        OperatingActionKind::Accept,
        false,
    )?;
    ensure!(task.landed.is_none(), "task already has a landing receipt");
    ensure!(
        run.review
            .as_ref()
            .is_none_or(|r| r.verdict.is_some() || r.cancelled),
        "unfinished native review must be resolved first"
    );
    ensure!(
        run.gate
            .as_ref()
            .is_none_or(|g| g.result != GateResult::Running),
        "unfinished native gate must be resolved first"
    );
    ensure!(
        task.launch
            .as_ref()
            .is_none_or(|l| l.stage != LaunchStage::Submitting),
        "unfinished dispatch submission must be reconciled first"
    );
    // This is deliberately read-only. Never reconcile, publish or discard another landing.
    crate::plan_target::validate_run(&ctx.store, task, &RunHead::of(run)?)?;
    let repo = ctx
        .store
        .project_path(
            task.project
                .as_deref()
                .context("external completion requires a project")?,
        )?
        .canonicalize()
        .context("resolving external destination repository")?;
    crate::plan_target::require_no_unfinished(&repo)?;
    let branch = crate::plan_target::branch(&ctx.store, task)?.unwrap_or_else(|| "main".into());
    ensure!(
        run.base == branch,
        "configured destination differs from the frozen run destination"
    );
    for op in crate::hooks::post_land_operations(ctx, None)? {
        ensure!(
            op.repo != repo || op.abandoned || op.ready(),
            "conflicting unfinished integration {}; reconcile it first",
            op.key
        );
    }
    for record in records(ctx, &task.id)? {
        ensure!(
            matches!(
                state(&record)?,
                RecoveryState::Applied | RecoveryState::Failed
            ),
            "unfinished recovery {}; reconcile it first",
            record.id
        );
    }
    for object in [&delivery.commit, &delivery.tree] {
        ensure!(
            (object.len() == 40 || object.len() == 64)
                && object
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "external commit/tree must be full lowercase object IDs"
        );
    }
    ensure!(
        corral_git::rev_sha(&repo, &format!("{}^{{commit}}", delivery.commit))? == delivery.commit,
        "external delivered commit mismatch"
    );
    ensure!(
        corral_git::rev_sha(&repo, &format!("{}^{{tree}}", delivery.commit))? == delivery.tree,
        "external delivered tree mismatch"
    );
    let head = corral_git::rev_sha(&repo, &format!("refs/heads/{branch}"))?;
    ensure!(
        corral_git::is_ancestor(&repo, &delivery.commit, &head)?,
        "external delivered commit is not reachable on configured destination {branch}"
    );
    for proof in [&delivery.gate, &delivery.review, &delivery.landing] {
        let artifact = crate::artifact_storage::resolve_artifact(ctx, &proof.reference)?;
        ensure!(
            artifact
                .path
                .starts_with(&format!("artifacts/{}/", task.id))
                && artifact.path == proof.reference,
            "external evidence requires an exact stored artifact path owned by this task"
        );
        ensure!(
            artifact.sha256 == proof.sha256
                && crate::verification::digest(&crate::artifact_storage::artifact_read(
                    ctx, &artifact
                )?) == proof.sha256,
            "external evidence digest mismatch"
        );
    }
    Ok(ExternalDestination {
        tree: corral_git::rev_sha(&repo, &format!("{head}^{{tree}}"))?,
        repo,
        branch,
        head,
    })
}

/// One canonical transaction: no Git change, process action, cleanup, repair credit or driver resume.
pub(super) fn apply(
    ctx: &Ctx,
    input: &RecoveryInput,
    mut record: RecoveryRecord,
) -> Result<RecoveryRecord> {
    let id = record.preview.request.target.task.clone();
    let RecoveryEffect::ExternalCompletion {
        run,
        delivery,
        destination,
    } = record.preview.proposed_effect.clone()
    else {
        anyhow::bail!("invalid external completion effect")
    };
    let _approval = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("approval already in progress")?;
    let _landing = corral_git::landing_lock(&destination.repo)?;
    let now = corral_core::clock::now();
    let provenance = ExternalLanding {
        recovery: reference(&record),
        destination: destination.clone(),
        tree: delivery.tree.clone(),
    };
    record.outcomes.push(outcome(RecoveryState::Applied, "External/forced completion verified; no native merge, gate, review or process drain claimed", Some(record.preview.proposed_effect.clone())));
    ctx.store.dispatch_update_recovery(&id, &record, |store, task| {
        let mut local = ctx.clone();
        local.store = store.clone();
        current_preview(&local, &record.preview, input)?;
        let current = task.current_run_mut().filter(|r| r.id == run).context("external run replaced")?;
        current.ended.get_or_insert(now);
        current.outcome = Some(RunOutcome::Done);
        task.status = Status::Done;
        task.queued = false;
        task.landed = Some(LandingReceipt { run: run.clone(), sha: delivery.commit.clone(), external: Some(provenance.clone()) });
        store.log_event(Some(&id), "approved", serde_json::json!({
            "run":run, "sha":delivery.commit, "base":destination.branch, "project":task.project,
            "strategy":"external_forced", "acceptance_mode":"external_forced", "policy_satisfied":false,
            "external":delivery, "destination":destination, "recovery":reference(&record),
            "reason":record.authorization.reason, "checks":record.preview.checks,
            "worktree_removed":false, "branch_deleted":false, "process_drain":"not_claimed",
            "cleanup":{"v":1,"remove_worktree":false}
        }))?;
        Ok(())
    })?;
    super::apply::project_event(ctx, &id, &record)?;
    Ok(record)
}
