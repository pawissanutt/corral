//! Host-owned repair admission. Reports are observations, not counter/reset instructions.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    AttemptedFix, FailureIdentity, FindingId, FindingMatch, FindingResolution, GateResult,
    RepairAttemptId, RepairAttemptKind, RepairDiagnosis, RepairEvent, RepairFinding, RepairHistory,
    RepairLedger, RepairStartReceipt, RepairTransition, Store, TaskFieldsRef as _, TaskHead,
    TaskRef, Verdict,
};
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingReport {
    pub observation_id: String,
    pub identity: FailureIdentity,
    pub observed_failure: String,
    #[serde(default)]
    pub attempted_fixes: Vec<AttemptedFix>,
    pub resolution: FindingResolution,
}

pub fn repair_status(ctx: &Ctx, id: &str) -> Result<Option<RepairLedger>> {
    ctx.store.repair_ledger(&Store::normalize_id(id)?)
}

fn initial(history: &mut RepairHistory, run: &str) -> Result<()> {
    if history.counters()?.initial_implementations == 0 {
        history.apply(RepairEvent::Started {
            receipt: RepairStartReceipt {
                attempt_id: RepairAttemptId(format!("initial:{run}")),
                profile: history.current_profile().into(),
                kind: RepairAttemptKind::InitialImplementation,
                findings: Vec::new(),
            },
        })?;
    }
    Ok(())
}

fn load(store: &Store, task: &impl TaskRef) -> Result<Option<RepairLedger>> {
    if let Some(ledger) = store.repair_ledger(task.id())? {
        return Ok(Some(ledger));
    }
    let config = store.config_layers()?.config()?;
    let Some(policy) = config.routing.repair.clone() else {
        return Ok(None);
    };
    let run = task.current_run_head()?.context("repair has no run")?;
    // Reuse pre-runtime contract records, never adopt a new allowance over old counters.
    let history = match store.record("profile-repairs", task.id())? {
        Some(history) => history,
        None => {
            let mut history = RepairHistory::new(
                policy,
                &run.agent,
                config
                    .profiles
                    .get(&run.agent)
                    .and_then(|p| p.escalate_to.clone()),
            )?;
            if *task.rounds() > 0
                || task.runs_len()? > 1
                || run.changes_round.as_ref().is_some_and(|r| r.n > 0)
            {
                initial(&mut history, &run.id)?;
                history.apply(RepairEvent::CodingBlocked { reason:
                    "Corrective history predates staged accounting; reconcile it and obtain explicit owner renewal, never silently reset legacy attempts".into() })?;
            }
            history
        }
    };
    Ok(Some(RepairLedger::new(history)))
}

pub(crate) fn enabled(store: &Store, task: &impl TaskRef) -> Result<bool> {
    Ok(store.config_layers()?.config()?.routing.repair.is_some()
        || store.repair_ledger(task.id())?.is_some())
}

fn observe(ledger: &mut RepairLedger, report: FindingReport) -> Result<()> {
    let h = &mut ledger.active_mut()?.history;
    if let Some(old) = h.events().iter().find_map(|event| match event {
        RepairEvent::Finding { finding } if finding.observation_id == report.observation_id => {
            Some(finding)
        }
        _ => None,
    }) {
        ensure!(
            old.identity == report.identity
                && old.observed_failure == report.observed_failure
                && old.attempted_fixes == report.attempted_fixes
                && old.resolution == report.resolution,
            "conflicting finding observation ID"
        );
        return Ok(());
    }
    let previous = h
        .findings()
        .into_iter()
        .find(|f| f.identity == report.identity);
    let (finding_id, matching) = match previous {
        Some(previous) => (
            previous.finding_id.clone(),
            FindingMatch::Exact {
                finding: previous.finding_id.clone(),
            },
        ),
        None => (FindingId(report.identity.fingerprint()), FindingMatch::New),
    };
    h.apply(RepairEvent::Finding {
        finding: RepairFinding {
            fingerprint: report.identity.fingerprint(),
            identity: report.identity,
            observation_id: report.observation_id,
            finding_id,
            matching,
            observed_failure: report.observed_failure,
            attempted_fixes: report.attempted_fixes,
            resolution: report.resolution,
        },
    })?;
    Ok(())
}

/// Structured input can describe identity, fixes and evidence, but not allowance or stage.
pub fn repair_finding(ctx: &Ctx, id: &str, report: FindingReport) -> Result<RepairLedger> {
    let id = Store::normalize_id(id)?;
    let _observation = ctx
        .store
        .try_lock(&format!("repair-observe-{id}"))?
        .context("another repair observation is being recorded")?;
    ctx.store.records_transaction(|| {
        let task = ctx.store.head(&id)?;
        let mut ledger = load(&ctx.store, &task)?.context("staged repair policy is not adopted")?;
        let run = task.current_run_head()?.context("missing run")?;
        initial(&mut ledger.active_mut()?.history, &run.id)?;
        observe(&mut ledger, report)?;
        ctx.store.save_repair_ledger(&id, &ledger)
    })?;
    ctx.store
        .repair_ledger(&id)?
        .context("repair ledger missing")
}

/// Persist machine observations before choosing a route. Gate failures use the exact failed
/// command identity. Unstructured reviews are explicitly uncertain, never prose-equality repeats.
pub(crate) fn observations(
    ctx: &Ctx,
    head: &TaskHead,
    extra: Option<(&str, &str)>,
) -> Result<Option<RepairLedger>> {
    if !enabled(&ctx.store, head)? {
        return Ok(None);
    }
    let _observation = ctx
        .store
        .try_lock(&format!("repair-observe-{}", head.id))?
        .context("another repair observation is being recorded")?;
    ctx.store.records_transaction(|| {
        let task = ctx.store.head(&head.id)?;
        let run = task.current_run_head()?.context("missing run")?;
        ensure!(head.current_run.as_ref().is_some_and(|r| r.id == run.id), "repair run changed");
        let mut ledger = load(&ctx.store, &task)?.context("repair policy missing")?;
        initial(&mut ledger.active_mut()?.history, &run.id)?;
        let correction = ledger.active()?.history.events().iter().rev().find_map(|event| match event {
            RepairEvent::Started { receipt } if matches!(receipt.kind, RepairAttemptKind::InitialImplementation
                | RepairAttemptKind::CurrentProfileRepair | RepairAttemptKind::StrongerProfileRepair) => Some(receipt.attempt_id.0.clone()),
            _ => None,
        }).context("missing coding receipt")?;
        if let Some(gate) = run.gate.as_ref().filter(|g| matches!(g.result, GateResult::Fail | GateResult::Stale)) {
            let command = gate.failed_cmd.as_deref().unwrap_or("unknown gate command");
            observe(&mut ledger, FindingReport {
                observation_id: format!("gate:{}:{}:{}:{correction}", run.id, gate.sha, gate.at),
                identity: FailureIdentity { criterion_or_test: command.into(),
                    affected_contract_or_location: "configured gate".into(), failure_code: gate.result.as_str().into() },
                observed_failure: format!("{}: {command}", gate.result.as_str()),
                attempted_fixes: Vec::new(), resolution: FindingResolution::Unresolved,
            })?;
        }
        if let Some(review) = run.review.as_ref().filter(|r| r.verdict == Some(Verdict::Changes)) {
            let observation_id = format!("review:{}:{}:{}:{correction}", run.id, review.sha, review.at);
            if !ledger.active()?.history.events().iter().any(|event| matches!(event,
                RepairEvent::Finding { finding } if finding.observation_id == observation_id)) {
                let h = &mut ledger.active_mut()?.history;
                let candidates = h.findings().into_iter().filter(|f| !matches!(f.resolution,
                    FindingResolution::Resolved { .. })).map(|f| f.finding_id.clone()).collect::<Vec<_>>();
                let identity = FailureIdentity { criterion_or_test: "unstructured review finding".into(),
                    affected_contract_or_location: "implementation review contract".into(), failure_code: observation_id.clone() };
                h.apply(RepairEvent::Finding { finding: RepairFinding {
                    finding_id: FindingId(identity.fingerprint()), fingerprint: identity.fingerprint(), identity,
                    observation_id, observed_failure: review.notes.clone().unwrap_or_else(|| "changes verdict".into()),
                    matching: if candidates.is_empty() { FindingMatch::New } else { FindingMatch::Uncertain {
                        candidates, reason: "Reviewer supplied no machine identity; semantic matching remains unverified".into() } },
                    attempted_fixes: Vec::new(), resolution: FindingResolution::Unresolved,
                } })?;
            }
        }
        if ledger.unresolved()?.is_empty() && let Some((key, details)) = extra {
            observe(&mut ledger, FindingReport { observation_id: format!("request:{key}"),
                identity: FailureIdentity { criterion_or_test: key.into(), affected_contract_or_location: "repair request".into(),
                    failure_code: "corrective_request".into() }, observed_failure: details.into(),
                attempted_fixes: Vec::new(), resolution: FindingResolution::Unresolved })?;
        }
        ctx.store.save_repair_ledger(&head.id, &ledger)
    })?;
    ctx.store.repair_ledger(&head.id)
}

pub(crate) fn transition(
    ctx: &Ctx,
    head: &TaskHead,
    ledger: &RepairLedger,
    explicit: bool,
) -> Result<RepairTransition> {
    let next = ledger.transition()?;
    let config = ctx.store.config_layers()?.config()?;
    let run = head.current_run.as_ref().context("missing run")?;
    let blocker = match &next {
        RepairTransition::Stronger { .. } if head.cowork_run().is_some() => Some("Co-work repair cannot silently replace the owner's assistant; propose an owner-directed handoff".into()),
        RepairTransition::Stronger { profile } if profile != &run.agent && !explicit && (head.profile.is_some()
            || run.route.as_ref().is_some_and(|r| r.is_manual())) => Some("manual implementation pin blocks automatic stronger profile".into()),
        RepairTransition::Current { profile } | RepairTransition::Stronger { profile } => {
            profile_blocker(ctx, &config, head, profile, explicit)
        }
        RepairTransition::Diagnose if ledger.active()?.history.stronger_profile().is_none()
            && ledger.active()?.history.coding_blocker().is_none() => Some("No configured stronger repair profile; unused allowance cannot be reassigned".into()),
        _ => None,
    };
    if let Some(reason) = blocker {
        ctx.store.update_head(&head.id, |_| {
            let mut current = ctx
                .store
                .repair_ledger(&head.id)?
                .context("repair ledger missing")?;
            ensure!(
                current.digest()? == ledger.digest()?,
                "repair observations changed before transition"
            );
            current
                .active_mut()?
                .history
                .apply(RepairEvent::CodingBlocked { reason })?;
            ctx.store.save_repair_ledger(&head.id, &current)
        })?;
        return ctx
            .store
            .repair_ledger(&head.id)?
            .context("repair ledger missing")?
            .transition();
    }
    Ok(next)
}

fn profile_blocker(
    ctx: &Ctx,
    config: &corral_core::Config,
    head: &TaskHead,
    profile: &str,
    explicit: bool,
) -> Option<String> {
    let check = || -> Result<Option<String>> {
        config.check_profile_eligibility(
            profile,
            corral_core::ProfileRole::Implementation,
            head.project.as_deref(),
            !explicit,
        )?;
        let resolved = config.resolve_profile(
            &corral_core::ProfileChoice {
                profile: Some(profile.into()),
                ..Default::default()
            },
            None,
            None,
        )?;
        if let Some(metadata) = config
            .profiles
            .get(profile)
            .and_then(|p| p.metadata.as_ref())
            && !metadata.candidates.is_empty()
            && metadata.fallback.is_some()
        {
            let mut check = |candidate: &corral_core::ResolvedProfile| {
                crate::routing::known_model_unavailability(ctx, candidate)
            };
            return Ok(
                match crate::availability::before_start(config, profile, metadata, None, &mut check)
                {
                    crate::availability::Start::Exhausted(reason) => Some(reason),
                    _ => None,
                },
            );
        }
        Ok(crate::routing::known_model_unavailability(ctx, &resolved))
    };
    match check() {
        Ok(reason) => reason,
        Err(error) => Some(format!("repair profile unavailable: {error:#}")),
    }
}

pub(crate) fn next_profile(ctx: &Ctx, head: &TaskHead, explicit: bool) -> Result<Option<String>> {
    let Some(ledger) = observations(
        ctx,
        head,
        Some(("corrective retry", "A corrective retry was requested")),
    )?
    else {
        return Ok(None);
    };
    match transition(ctx, head, &ledger, explicit)? {
        RepairTransition::Current { profile } | RepairTransition::Stronger { profile } => {
            Ok(Some(profile))
        }
        next => bail!(
            "repair requires {next:?}; inspect `corral repair status {}`",
            head.id
        ),
    }
}

#[derive(Serialize, Deserialize)]
struct Continuation {
    previous: String,
    next: String,
    availability: bool,
}

pub(crate) fn prepare_continuation(
    store: &Store,
    task: &impl TaskRef,
    previous: &str,
    availability: bool,
) -> Result<()> {
    if enabled(store, task)? {
        store.save_record(
            "repair-continuation",
            task.id(),
            &Continuation {
                previous: previous.into(),
                next: format!("R{}", task.runs_len()? + 1),
                availability,
            },
        )?;
    }
    Ok(())
}

pub(crate) fn start_launch(store: &Store, task: &impl TaskRef) -> Result<()> {
    let run = task.current_run_head()?.context("missing launch run")?;
    let continuation: Option<Continuation> = store.record("repair-continuation", task.id())?;
    if let Some(pending) = &continuation {
        ensure!(
            pending.next == run.id && pending.previous != run.id,
            "corrective continuation is bound to another run"
        );
    }
    start(
        store,
        task,
        &format!("launch:{}", run.id),
        &run.agent,
        continuation.as_ref().is_some_and(|c| c.availability),
    )?;
    // A recovery marker is single-use. It cannot exempt an unrelated future retry from
    // coding accounting. Removal commits atomically with the launch receipt and run.
    if continuation.is_some() {
        store.delete_record("repair-continuation", task.id())?;
    }
    Ok(())
}

/// Called in the executor's final durable prepare transaction, before any IPC. An uncertain
/// submission retains its receipt and cannot spend again. The initial launch is not a repair.
pub(crate) fn start(
    store: &Store,
    task: &impl TaskRef,
    attempt: &str,
    profile: &str,
    availability: bool,
) -> Result<()> {
    let Some(mut ledger) = load(store, task)? else {
        return Ok(());
    };
    let run = task.current_run_head()?.context("missing run")?;
    ensure!(
        profile == run.agent,
        "repair receipt profile differs from the prepared executor"
    );
    let h = &mut ledger.active_mut()?.history;
    let first = h.counters()?.initial_implementations == 0;
    if first {
        initial(h, &run.id)?;
    }
    let id = RepairAttemptId(attempt.into());
    if let Some(old) = h.events().iter().find_map(|e| match e {
        RepairEvent::Started { receipt } if receipt.attempt_id == id => Some(receipt),
        _ => None,
    }) {
        ensure!(
            old.profile == profile
                && availability
                    == matches!(old.kind, RepairAttemptKind::AvailabilityRecovery { .. }),
            "conflicting corrective attempt identity"
        );
        return Ok(());
    }
    if ledger.active()?.history.coding_blocker().is_none()
        && attempt == format!("launch:{}", run.id) && ledger.active()?.history.events().iter().any(|event|
        matches!(event, RepairEvent::Started { receipt } if receipt.kind == RepairAttemptKind::InitialImplementation
            && receipt.attempt_id.0 == format!("initial:{}", run.id))) {
        return store.save_repair_ledger(task.id(), &ledger);
    }
    let findings = ledger.unresolved()?;
    let kind = if availability {
        let interrupted = ledger
            .active()?
            .history
            .events()
            .iter()
            .rev()
            .find_map(|e| match e {
                RepairEvent::Started { receipt }
                    if matches!(
                        receipt.kind,
                        RepairAttemptKind::InitialImplementation
                            | RepairAttemptKind::CurrentProfileRepair
                            | RepairAttemptKind::StrongerProfileRepair
                    ) =>
                {
                    Some(receipt.attempt_id.clone())
                }
                _ => None,
            })
            .context("availability recovery has no interrupted attempt")?;
        RepairAttemptKind::AvailabilityRecovery {
            interrupted_attempt: interrupted,
        }
    } else {
        match ledger.transition()? {
            RepairTransition::Current { profile: expected } if expected == profile => {
                RepairAttemptKind::CurrentProfileRepair
            }
            RepairTransition::Stronger { profile: expected } if expected == profile => {
                RepairAttemptKind::StrongerProfileRepair
            }
            next => bail!("repair transition {next:?} does not authorize profile {profile}"),
        }
    };
    // Recovery retains the exact addressed findings, including when reports arrived later.
    let findings = if let RepairAttemptKind::AvailabilityRecovery {
        interrupted_attempt,
    } = &kind
    {
        ledger
            .active()?
            .history
            .events()
            .iter()
            .find_map(|e| match e {
                RepairEvent::Started { receipt } if &receipt.attempt_id == interrupted_attempt => {
                    Some(receipt.findings.clone())
                }
                _ => None,
            })
            .context("interrupted receipt missing")?
    } else {
        findings
    };
    ledger.active_mut()?.history.apply(RepairEvent::Started {
        receipt: RepairStartReceipt {
            attempt_id: id,
            profile: profile.into(),
            kind,
            findings,
        },
    })?;
    store.save_repair_ledger(task.id(), &ledger)?;
    store.log_event(
        Some(task.id()),
        "repair_attempt_started",
        serde_json::json!({"attempt":attempt,
        "profile":profile,"at":store.now(),"counters":ledger.active()?.history.counters()?}),
    )
}

pub(crate) fn require_coding<T: TaskRef + ?Sized>(store: &Store, task: &T) -> Result<()> {
    if let Some(ledger) = store.repair_ledger(task.id())? {
        ensure!(
            matches!(
                ledger.transition()?,
                RepairTransition::Current { .. } | RepairTransition::Stronger { .. }
            ),
            "coding repair allowance exhausted; PM diagnosis or explicit owner renewal required"
        );
    }
    Ok(())
}

pub fn repair_diagnose(ctx: &Ctx, id: &str) -> Result<RepairLedger> {
    diagnose(ctx, id, false)
}

pub fn repair_diagnose_owner(ctx: &Ctx, id: &str) -> Result<RepairLedger> {
    crate::require_automation_owner(&ctx.store)?;
    diagnose(ctx, id, true)
}

fn diagnose(ctx: &Ctx, id: &str, owner: bool) -> Result<RepairLedger> {
    let id = Store::normalize_id(id)?;
    ctx.store.dispatch_update_head(&id, |store, task| {
        crate::hold::require_clear(store, &*task)?;
        // Diagnosis uses the delegated repair scope, not the coding allowance. No launch,
        // write, gate, acceptance or enlarged budget is authorized by this operation.
        crate::operating::require_diagnosis(store, &*task, owner)?;
        ensure!(matches!(task.status, corral_core::Status::Review | corral_core::Status::NeedsMe)
            && task.current_run_head()?.is_some_and(|run| run.outcome == Some(corral_core::RunOutcome::Done))
            && task.launch.is_none(), "diagnosis requires a completed corrective attempt, not an active or queued writer");
        let mut ledger = ctx.store.repair_ledger(&id)?.context("no repair history")?;
        if ledger.active()?.diagnosis.is_some() {
            return Ok(());
        }
        ensure!(
            ledger.transition()? == RepairTransition::Diagnose,
            "PM diagnosis is not the next stage"
        );
        let observed_after_coding = ledger.active()?.history.events().iter().rev().take_while(|event|
            !matches!(event, RepairEvent::Started { receipt } if matches!(receipt.kind,
                RepairAttemptKind::InitialImplementation | RepairAttemptKind::CurrentProfileRepair | RepairAttemptKind::StrongerProfileRepair)))
            .any(|event| matches!(event, RepairEvent::Finding { finding } if !matches!(finding.resolution, FindingResolution::Resolved { .. })));
        ensure!(observed_after_coding, "record an unresolved finding after the last coding attempt before diagnosis");
        let attempt_id = RepairAttemptId(format!("diagnosis:{}", ledger.allowances.len()));
        let findings = ledger.unresolved()?;
        let active = ledger.active_mut()?;
        active.history.apply(RepairEvent::Started {
            receipt: RepairStartReceipt {
                attempt_id: attempt_id.clone(),
                profile: ctx.store.config_layers()?.config()?.pm.agent,
                kind: RepairAttemptKind::PmDiagnosis,
                findings,
            },
        })?;
        active.diagnosis = Some(RepairDiagnosis {
            id: attempt_id.0,
            started: ctx.store.now(),
            proposal: None,
            evidence: Vec::new(),
        });
        ctx.store.save_repair_ledger(&id, &ledger)?;
        ctx.store.log_event(
            Some(&id),
            "repair_diagnosis_started",
            serde_json::json!({"at":ctx.store.now()}),
        )
    })?;
    ctx.store
        .repair_ledger(&id)?
        .context("repair ledger missing")
}

pub fn repair_diagnosed(
    ctx: &Ctx,
    id: &str,
    diagnosis: &str,
    proposal: &str,
    evidence: Vec<String>,
) -> Result<RepairLedger> {
    ensure!(
        !proposal.trim().is_empty()
            && !evidence.is_empty()
            && evidence.iter().all(|e| !e.trim().is_empty()),
        "diagnosis requires a concrete bounded proposal and evidence"
    );
    let id = Store::normalize_id(id)?;
    ctx.store.update_head(&id, |task| {
        let mut ledger = ctx.store.repair_ledger(&id)?.context("no repair history")?;
        let active = ledger.active_mut()?;
        let record = active
            .diagnosis
            .as_mut()
            .context("diagnosis has not started")?;
        ensure!(record.id == diagnosis, "stale diagnosis identity");
        if record.proposal.is_some() {
            ensure!(
                record.proposal.as_deref() == Some(proposal) && record.evidence == evidence,
                "conflicting diagnosis result"
            );
            return Ok(());
        }
        record.proposal = Some(proposal.into());
        record.evidence = evidence;
        ctx.store.save_repair_ledger(&id, &ledger)?;
        task.status = corral_core::Status::NeedsMe;
        ctx.store.log_event(
            Some(&id),
            "repair_diagnosed",
            serde_json::json!({"diagnosis":diagnosis,"proposal":proposal}),
        )
    })?;
    ctx.store
        .repair_ledger(&id)?
        .context("repair ledger missing")
}

pub fn repair_renew(
    ctx: &Ctx,
    id: &str,
    renewal: corral_core::RepairRenewal,
    policy: corral_core::RepairPolicy,
) -> Result<RepairLedger> {
    crate::require_automation_owner(&ctx.store)?;
    let id = Store::normalize_id(id)?;
    let head = ctx.store.head(&id)?;
    ensure!(
        head.launch.is_none(),
        "reconcile prepared launch before renewal"
    );
    let run = head
        .current_run
        .as_ref()
        .context("no implementation to renew")?;
    let config = ctx.store.config_layers()?.config()?;
    ctx.store.renew_repair_allowance(
        &id,
        renewal,
        policy,
        &run.agent,
        config
            .profiles
            .get(&run.agent)
            .and_then(|p| p.escalate_to.clone()),
    )
}

#[cfg(test)]
#[path = "repair_runtime_tests.rs"]
mod tests;
