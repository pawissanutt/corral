//! Reconcile one already-authorized ref advancement. Never run landing, cleanup or hook commands.
use super::*;
use std::path::PathBuf;

#[derive(PartialEq, Eq)]
struct RunBinding {
    id: String,
    branch: String,
    base: String,
    base_sha: String,
    worktree: PathBuf,
    plan_target: Option<RunPlanTarget>,
}
impl RunBinding {
    fn of(run: &RunHead) -> Self {
        Self {
            id: run.id.clone(),
            branch: run.branch.clone(),
            base: run.base.clone(),
            base_sha: run.base_sha.clone(),
            worktree: run.worktree.clone(),
            plan_target: run.plan_target.clone(),
        }
    }
}

fn effect(record: &RecoveryRecord) -> Result<(&CandidateIdentity, RecoveryMerge)> {
    let RecoveryEffect::CodeAcceptance { candidate, merge } = &record.preview.proposed_effect
    else {
        anyhow::bail!("not code acceptance")
    };
    Ok((candidate, *merge))
}
fn strategy(merge: RecoveryMerge) -> &'static str {
    match merge {
        RecoveryMerge::Ff => "fast_forward",
        RecoveryMerge::Rebase => "rebase",
        RecoveryMerge::Keep => "keep_branch",
    }
}

enum Journal {
    Main(Box<crate::hooks::PostLandOperation>),
    Plan(Box<crate::plan_target::Target>, String),
}

/// Opaque proof required to recompute identity while a proven Plan landing is still unpublished.
/// Only this module constructs it, after checking the exact reverse intent and owning journal.
pub(crate) struct LandingProof {
    task: String,
    run: RunBinding,
    candidate: CandidateIdentity,
}
impl LandingProof {
    pub(crate) fn matches<T: TaskRef + ?Sized>(
        &self,
        task: &T,
        run: &RunHead,
        candidate: &CandidateIdentity,
    ) -> bool {
        task.id() == &self.task && RunBinding::of(run) == self.run && *candidate == self.candidate
    }
}

fn journal(
    ctx: &Ctx,
    task: &Task,
    run: &RunHead,
    record: &RecoveryRecord,
    repo: &std::path::Path,
) -> Result<Option<(Journal, LandingProof)>> {
    let Some((link, saved_effect, key)): Option<(
        RecoveryReference,
        RecoveryEffect,
        Option<String>,
    )> = corral_core::records::RecordArea::new("recovery-landings").load(&ctx.store, &record.id)?
    else {
        return Ok(None);
    };
    let (candidate, merge) = effect(record)?;
    ensure!(
        link == reference(record) && saved_effect == record.preview.proposed_effect,
        "recovery landing intent differs"
    );
    // Exact advancement only: ancestry or Done alone must not borrow another landing's authority.
    if candidate.head == candidate.integration_base
        || corral_git::rev_sha(repo, &run.base)? != candidate.head
    {
        return Ok(None);
    }
    let journal = if run.plan_target.is_some() {
        let (target, actual) =
            crate::plan_target::recovery_landing(ctx, task, run, candidate, strategy(merge))?;
        ensure!(
            key.as_ref().is_none_or(|key| *key == actual),
            "Plan landing journal differs"
        );
        corral_git::check_target_checkout(repo, &target.checkout, &run.base, &candidate.head)?;
        Journal::Plan(Box::new(target), actual)
    } else if let Some(key) = key {
        let op =
            crate::hooks::landing_operation(ctx, &key)?.context("bound landing journal missing")?;
        ensure!(
            op.version == 1
                && op.key == key
                && op.task == task.id
                && op.run == run.id
                && op.repo == repo
                && op.base == run.base
                && op.base_before == candidate.integration_base
                && op.landed_sha == candidate.head
                && !op.abandoned,
            "bound landing journal differs"
        );
        Journal::Main(Box::new(op))
    } else {
        // No owning journal, no proof: ref position alone cannot establish delivery.
        return Ok(None);
    };
    Ok(Some((
        journal,
        LandingProof {
            task: task.id.clone(),
            run: RunBinding::of(run),
            candidate: candidate.clone(),
        },
    )))
}

fn check_receipts(
    ctx: &Ctx,
    task: &Task,
    run: &RunHead,
    record: &RecoveryRecord,
    candidate: &CandidateIdentity,
) -> Result<()> {
    for check in &record.preview.checks {
        match &check.observed {
            RecoveryObservation::Gate { result } if check.check == RecoveryCheck::GateOutcome => {
                let gate = run.gate.as_ref().context("landing gate missing")?;
                ensure!(
                    gate.result == *result
                        && matches!(result, GateResult::Pass | GateResult::Fail)
                        && gate.identity.as_ref() == Some(candidate)
                        && gate.sha == candidate.head
                        && crate::gate::gate_log_valid(ctx, task, run)?,
                    "landing gate receipt changed or unverifiable"
                );
            }
            RecoveryObservation::Review { verdict }
                if check.check == RecoveryCheck::ReviewOutcome =>
            {
                let review = run.review.as_ref().context("landing review missing")?;
                ensure!(
                    verdict.is_some()
                        && review.verdict == *verdict
                        && review.identity.as_ref() == Some(candidate)
                        && review.sha == candidate.head
                        && review.evidence_digest.as_deref()
                            == Some(crate::verification::evidence_digest(run)?.as_str()),
                    "landing review receipt changed or unverifiable"
                );
            }
            _ => {}
        }
    }
    if task.completion.as_ref().is_some_and(|manifest| {
        manifest.audited
            || manifest.criteria.iter().any(|criterion| {
                run.progress
                    .as_ref()
                    .is_some_and(|progress| progress.criterion_complete(&criterion.id))
            })
    }) {
        let mut failures = crate::completion::manifest_failures(task, run)?;
        let admission = crate::completion::Admission {
            identity: candidate,
            start: crate::completion::attempt_start(run),
            end: None,
        };
        failures.extend(crate::completion::obligation_failures(
            ctx, task, run, &admission, None,
        )?);
        ensure!(
            failures.is_empty(),
            "landing evidence changed: {}",
            failures.join("; ")
        );
    }
    Ok(())
}

pub(super) fn reconcile(ctx: &Ctx, record: &RecoveryRecord) -> Result<bool> {
    let (candidate, merge) = effect(record)?;
    let committed = super::code::code_committed(ctx, record)?;
    if merge == RecoveryMerge::Keep {
        return Ok(committed);
    }
    let id = &record.preview.request.target.task;
    let _approval = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .context("approval still in progress")?;
    let task = ctx.store.get(id)?;
    let run = RunHead::of(task.current_run().context("landing run missing")?)?;
    let repo = crate::review::main_repo(ctx, &task, &run)?;
    let _landing = corral_git::landing_lock(&repo)?;
    let mut observed = None;
    ctx.store.dispatch_update_head(id, |store, head| {
        observed = None;
        let mut local = ctx.clone(); local.store = store.clone();
        let task = head.shell_task()?;
        let run = head.current_run_head()?.context("landing run missing")?.into_owned();
        let Some((journal, proof)) = journal(&local, &task, &run, record, &repo)? else { return Ok(()) };
        if let Some(plan) = store.execution_plan_ref(&task)? {
            crate::plan_recovery::require_active(store, &plan)?;
        }
        if !committed {
            super::continuity::validate_landed_contract(&local, &task, record, &proof)?;
            crate::hold::require_clear(store, &task)?;
            ensure!(corral_git::is_clean(&run.worktree)?, "landing candidate has uncommitted changes");
            check_receipts(&local, &task, &run, record, candidate)?;
            ensure!((task.awaiting_owner() || task.status == Status::Done)
                && run.outcome == Some(RunOutcome::Done) && run.facts.pending_decision.is_none(), "landing lifecycle or owner decision changed");
            ensure!(crate::plan::plan_spec_revision(&task)? == record.preview.request.target.spec_revision, "landing task specification changed");
            let RecoveryIdentity::Code { gate_attempt, review_attempt, .. } = &record.preview.request.target.identity else { anyhow::bail!("code identity missing") };
            ensure!(run.gate.as_ref().and_then(|gate| gate.identity.as_ref().map(|identity| &identity.attempt)) == gate_attempt.as_ref()
                && run.review.as_ref().and_then(crate::reviewer::review_callback_token).as_ref() == review_attempt.as_ref(), "landing checks replaced");
            head.landed = Some(corral_core::LandingReceipt { run: run.id.clone(), sha: candidate.head.clone() });
            crate::reviewer::supersede_review_head(head, corral_core::clock::now())?;
            let current = head.current_run_mut().context("landing run disappeared")?;
            current.ended.get_or_insert(corral_core::clock::now());
            current.outcome = Some(RunOutcome::Done);
            current.agent_state = Some("done".into());
            current.pending_reply = None;
            current.blocked_excerpt = None;
            current.idle_since = None;
            current.herdr_idle_since = None;
            current.idle_no_report = false;
            current.stuck = None;
            head.status = Status::Done;
            head.queued = false;
            super::code::code_acceptance_committed(&local, record, &candidate.head)?;
        }
        let key = match &journal { Journal::Main(op) => Some(op.key.as_str()), Journal::Plan(_, key) => Some(key.as_str()) };
        if store.last_event_where(Some(id), &["approved"], Some(&run.id), |event| event.detail["recovery"]["operation"] == record.id)?.is_none() {
            store.log_event(Some(id), "approved", serde_json::json!({"run":run.id, "sha":candidate.head, "previous_sha":candidate.integration_base,
                "base":run.base,"branch":run.branch,"strategy":strategy(merge),"landing_receipt":key,
                "acceptance_mode":"owner_recovery","policy_satisfied":false,"recovery":reference(record),"cleanup":{"v":1,"remove_worktree":false},
                "worktree_removed":false,"branch_deleted":false,"reconciled":true}))?;
        }
        if let Journal::Main(op) = &journal { let mut op = op.clone(); op.landed = true; crate::hooks::save(&local, &op)?; }
        observed = Some(journal);
        Ok(())
    })?;
    let Some(journal) = observed else {
        return Ok(committed && run.plan_target.is_none());
    };
    if let Journal::Plan(mut target, _) = journal {
        target.reconcile(ctx)?;
    }
    Ok(true)
}
