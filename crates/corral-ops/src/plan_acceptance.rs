//! Aggregate Plan verification and final acceptance on the existing gate, evidence,
//! review and operating-policy machinery. Child acceptance never completes a Plan.
use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    AGGREGATE_RUN, AggregateAttempt, AggregateLandingState, EvidenceObservation, EvidenceRecord,
    EvidenceSource, GateRecord, GateResult, ObservationStatus, Status, Task,
};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::Ctx;
use crate::verification::digest;

mod accept;
mod capture;
mod gate;
mod review;

pub use accept::plan_accept;
pub use review::{AggregateVerdict, plan_aggregate_review, plan_aggregate_verdict};

/// Trackers have no coding runs, so their snapshots use the store's run-less label.
const TRACKER_ARTIFACT_RUN: &str = "R0";

/// Aggregate readiness of a Plan with acceptance criteria; absent for plain trackers.
#[derive(Debug, Serialize)]
pub struct AggregateProgress {
    /// Every required member is accepted and included; verification may start.
    pub ready: bool,
    pub blockers: Vec<String>,
    pub attempt: Option<String>,
    pub accepted: bool,
    /// The landed final acceptance receipt, once recorded.
    pub acceptance: Option<corral_core::AggregateAcceptance>,
}

pub(crate) fn progress(ctx: &Ctx, tracker: &Task) -> Option<AggregateProgress> {
    if !tracker.plan_acceptance_required() {
        return None;
    }
    let acceptance = tracker
        .aggregate_acceptance()
        .and_then(|attempt| attempt.acceptance.clone())
        .filter(|acceptance| acceptance.state == AggregateLandingState::Landed);
    let accepted = acceptance.is_some();
    let blockers = if accepted {
        Vec::new()
    } else {
        match capture::capture(ctx, &ctx.store, tracker, "readiness", None) {
            Ok(_) => Vec::new(),
            Err(error) => match error.downcast::<capture::Unmet>() {
                Ok(unmet) => unmet.0,
                Err(error) => vec![format!("{error:#}")],
            },
        }
    };
    Some(AggregateProgress {
        ready: !accepted && blockers.is_empty(),
        blockers,
        attempt: tracker
            .aggregate_attempts
            .last()
            .map(|attempt| attempt.id.clone()),
        accepted,
        acceptance,
    })
}

fn current<'a>(tracker: &'a Task, attempt: &str) -> Result<&'a AggregateAttempt> {
    let latest = tracker
        .aggregate_attempts
        .last()
        .with_context(|| format!("{} has no aggregate attempt", tracker.id))?;
    ensure!(
        latest.id == attempt,
        "{attempt} is not the current aggregate attempt of {} (current {})",
        tracker.id,
        latest.id
    );
    ensure!(
        latest.acceptance.is_none(),
        "{attempt} already has an acceptance"
    );
    Ok(latest)
}

/// Fail when moved main, head, spec, policy, members or inputs changed the identity.
fn ensure_fresh(attempt: &AggregateAttempt, actual: &capture::Candidate) -> Result<()> {
    let mut changes: Vec<_> = attempt
        .identity
        .differences(&actual.identity)
        .into_iter()
        .map(|change| format!("{} {} -> {}", change.field, change.expected, change.actual))
        .collect();
    if attempt.identity.environment != actual.identity.environment {
        changes.push("environment".into());
    }
    if attempt.members != actual.members {
        changes.push("members".into());
    }
    if !changes.is_empty() {
        bail!(
            "stale Plan proof for {}: {}; run `corral plan verify {}` again",
            attempt.id,
            changes.join(", "),
            attempt.identity.task
        );
    }
    Ok(())
}

fn fresh_candidate(ctx: &Ctx, tracker: &Task, attempt: &AggregateAttempt) -> Result<()> {
    let candidate = capture::capture(
        ctx,
        &ctx.store.reopen()?,
        tracker,
        &attempt.id,
        Some(&attempt.identity.environment),
    )?;
    ensure_fresh(attempt, &candidate)
}

/// Start one aggregate attempt and, for code Plans, run the configured gate on the combined
/// candidate in `<repo>/.worktrees/<plan>-verify`. Blocks until the gate finishes.
///
/// # Errors
/// Refuses unmet required work, accepted Plans, missing gates and concurrent verification.
pub fn plan_verify(ctx: &Ctx, id: &str) -> Result<AggregateAttempt> {
    ctx.store.check_runtime()?;
    let _lock = ctx
        .store
        .try_lock(&format!("plan-verify-{id}"))?
        .context("another Plan verification is in progress")?;
    let tracker = ctx.store.get(id)?;
    ensure!(
        !matches!(tracker.status, Status::Done | Status::Dropped),
        "{} is {}",
        tracker.id,
        tracker.status.label()
    );
    ensure!(
        tracker.aggregate_acceptance().is_none(),
        "{} already has an accepted aggregate attempt",
        tracker.id
    );
    let attempt_id = format!("A{}", tracker.aggregate_attempts.len() + 1);
    let repo = capture::repo(&ctx.store, &tracker)?;
    let verify = repo
        .join(".worktrees")
        .join(format!("{}-verify", tracker.id));
    let candidate = {
        let _landing = corral_git::landing_lock(&repo)?;
        let candidate = capture::capture(ctx, &ctx.store.reopen()?, &tracker, &attempt_id, None)?;
        if candidate.code {
            corral_git::verification_checkout(&repo, &verify, &candidate.identity.head)?;
        }
        candidate
    };
    let policy = crate::verification::policy(ctx, &tracker)?;
    if candidate.code {
        crate::verification::require_gate(&policy, &tracker)?;
    }
    let mut attempt = AggregateAttempt {
        id: attempt_id,
        target: candidate.target.clone(),
        code: candidate.code,
        members: candidate.members.clone(),
        started: corral_core::clock::now(),
        gate: candidate.code.then(|| GateRecord {
            identity: Some(candidate.identity.clone()),
            commands: Vec::new(),
            log_sha256: None,
            result: GateResult::Running,
            process_lost: false,
            failed_cmd: None,
            at: corral_core::clock::now(),
            sha: candidate.identity.head.clone(),
            pane: None,
            socket: None,
        }),
        identity: candidate.identity,
        evidence: Vec::new(),
        reviews: Vec::new(),
        acceptance: None,
    };
    ctx.store.update(id, |task| {
        ensure!(
            task.aggregate_attempts.len() == tracker.aggregate_attempts.len()
                && task.aggregate_acceptance().is_none(),
            "aggregate attempts changed during verification"
        );
        task.aggregate_attempts.push(attempt.clone());
        Ok(())
    })?;
    ctx.store.log_event(
        Some(id),
        "plan_verification_started",
        json!({"attempt": attempt.id, "identity": attempt.identity, "members": attempt.members}),
    )?;
    if !attempt.code {
        return Ok(attempt);
    }
    let mut record = gate::run(ctx, &attempt, &verify, &policy.gate)?;
    if record.result == GateResult::Pass
        && let Some(reason) = gate::checkout_changed(&verify, &attempt.identity.head)?
    {
        record.result = GateResult::Fail;
        record.failed_cmd = Some(format!("verification_checkout_changed: {reason}"));
    }
    if record.result == GateResult::Pass {
        let fresh = {
            let _landing = corral_git::landing_lock(&repo)?;
            fresh_candidate(ctx, &ctx.store.get(id)?, &attempt)
        };
        if let Err(error) = fresh {
            record.result = GateResult::Fail;
            record.failed_cmd = Some(format!("stale_result: {error:#}"));
        }
    }
    let saved = ctx.store.update(id, |task| {
        let stored = task
            .aggregate_attempts
            .iter_mut()
            .find(|stored| stored.id == attempt.id && stored.identity == attempt.identity)
            .context("aggregate attempt changed while its gate ran")?;
        stored.gate = Some(record.clone());
        Ok(())
    })?;
    attempt = saved
        .aggregate_attempts
        .iter()
        .find(|stored| stored.id == attempt.id)
        .cloned()
        .context("aggregate attempt missing after gate")?;
    let log = gate::paths(ctx, id, &attempt.id).log;
    if log.is_file() {
        crate::artifacts::add_artifact(
            ctx,
            id,
            &log,
            Some(corral_core::ArtifactKind::Log),
            Some(&format!(
                "Plan gate {}: {}",
                attempt.id,
                record.result.as_str()
            )),
        )?;
    }
    ctx.store.log_event(
        Some(id),
        "plan_gate",
        json!({"attempt": attempt.id, "result": record.result.as_str(), "failed_cmd": record.failed_cmd, "identity": attempt.identity}),
    )?;
    Ok(attempt)
}

/// Record one aggregate scenario observation against the current attempt.
/// `observation.criterion` is a Plan criterion key and `run` is `aggregate`.
///
/// # Errors
/// Rejects stale or superseded attempts, unknown criteria and artifacts, and running gates.
pub fn plan_evidence(
    ctx: &Ctx,
    id: &str,
    observation: &EvidenceObservation,
) -> Result<EvidenceRecord> {
    ctx.store.check_runtime()?;
    if observation.version != 1
        || observation.run != AGGREGATE_RUN
        || [
            &observation.key,
            &observation.action,
            &observation.environment,
            &observation.expected,
            &observation.observed,
            &observation.producer,
        ]
        .iter()
        .any(|field| field.trim().is_empty())
    {
        bail!("invalid_evidence: version 1, run \"{AGGREGATE_RUN}\" and nonblank fields required");
    }
    let tracker = ctx.store.get(id)?;
    let attempt = current(&tracker, &observation.attempt)?.clone();
    {
        let _landing = corral_git::landing_lock(&capture::repo(&ctx.store, &tracker)?)?;
        fresh_candidate(ctx, &tracker, &attempt)?;
    }
    let mut result = None;
    let saved = ctx.store.update(id, |task| {
        let stored = current(task, &attempt.id)?;
        ensure!(*stored == attempt, "aggregate attempt changed; retry");
        if let Some(existing) = stored
            .evidence
            .iter()
            .find(|record| record.observation.key == observation.key)
        {
            ensure!(
                existing.observation == *observation,
                "conflicting_idempotency_key"
            );
            result = Some(existing.clone());
            return Ok(());
        }
        let scope = task.approved_scope.as_ref().context("Plan scope missing")?;
        let index = scope
            .criterion_keys()
            .iter()
            .position(|key| *key == observation.criterion)
            .context("unknown_criterion")?;
        ensure!(
            observation.expected == scope.acceptance[index].expected,
            "criterion_expected_mismatch"
        );
        if matches!(&stored.gate, Some(gate) if gate.result == GateResult::Running) {
            bail!("gate_incomplete");
        }
        let earliest = earliest(stored);
        ensure!(
            observation.captured >= earliest && observation.captured <= corral_core::clock::now(),
            "capture_outside_attempt"
        );
        let artifact_hashes = snapshots(ctx, task, stored, &observation.artifacts)?;
        let record = EvidenceRecord {
            source: EvidenceSource::WorkerAssertion,
            observation: observation.clone(),
            identity: stored.identity.clone(),
            manifest_digest: digest(&serde_json::to_vec(&scope.acceptance)?),
            artifact_hashes,
        };
        task.aggregate_attempts
            .last_mut()
            .context("aggregate attempt missing")?
            .evidence
            .push(record.clone());
        result = Some(record);
        Ok(())
    })?;
    let record = result.context("evidence update produced no result")?;
    ctx.store.log_event(
        Some(&saved.id),
        "plan_evidence_recorded",
        json!({"attempt": attempt.id, "key": record.observation.key, "criterion": record.observation.criterion, "status": record.observation.status}),
    )?;
    Ok(record)
}

fn earliest(attempt: &AggregateAttempt) -> Timestamp {
    attempt
        .gate
        .as_ref()
        .map_or(attempt.started, |gate| gate.at)
}

/// The task evidence validator, applied to the tracker's own snapshots for this attempt.
fn snapshots(
    ctx: &Ctx,
    tracker: &Task,
    attempt: &AggregateAttempt,
    refs: &[String],
) -> Result<Vec<String>> {
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    crate::completion::snapshot_hashes(ctx, tracker, TRACKER_ARTIFACT_RUN, earliest(attempt), refs)
}

/// Recorded evidence whose snapshot bytes are missing, changed or captured before the attempt.
fn evidence_failures(ctx: &Ctx, tracker: &Task, attempt: &AggregateAttempt) -> Vec<String> {
    attempt
        .evidence
        .iter()
        .filter_map(|record| {
            let key = &record.observation.key;
            match snapshots(ctx, tracker, attempt, &record.observation.artifacts) {
                Ok(hashes) if hashes == record.artifact_hashes => None,
                Ok(_) => Some(format!("evidence {key}: artifact_tampered")),
                Err(error) => Some(format!("evidence {key}: {error:#}")),
            }
        })
        .collect()
}

/// Latest observation per criterion must pass; not-run, failing and missing are named.
fn scenario_failures(tracker: &Task, attempt: &AggregateAttempt) -> Vec<String> {
    let Some(scope) = &tracker.approved_scope else {
        return vec!["Plan scope missing".into()];
    };
    scope
        .criterion_keys()
        .into_iter()
        .filter_map(|key| {
            match attempt
                .evidence
                .iter()
                .rev()
                .find(|record| record.observation.criterion == key)
            {
                None => Some(format!("criterion {key} has no aggregate observation")),
                Some(record) if record.identity != attempt.identity => Some(format!(
                    "criterion {key} evidence is bound to another identity"
                )),
                Some(record) => match record.observation.status {
                    ObservationStatus::Pass => None,
                    ObservationStatus::Fail => Some(format!("criterion {key} is fail")),
                    ObservationStatus::NotRun => Some(format!("criterion {key} is not_run")),
                },
            }
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests;
