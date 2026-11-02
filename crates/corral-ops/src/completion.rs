//! Criterion registration, assertion ledger, and read-only completion intake.
use std::collections::HashSet;
use std::path::{Component, Path};
use std::process::Command;

use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{
    CandidateIdentity, CompletionAudit, CriteriaManifest, EvidenceObservation, EvidenceRecord,
    EvidenceSource, GateRecord, GateResult, ObservationStatus, RunHead, Task, TaskRef,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::agent::effects::{self, Receipted};
use corral_core::agent::{RequestOrigin, RequestReceipt};

use crate::gate_tier::GatePhase;
use crate::verification;

pub mod evidence_carry;

/// Current spec revision for constructing an explicit reviewed manifest.
pub fn completion_spec_digest(task: &Task) -> Result<String> {
    verification::spec_digest(task)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub(crate) fn manifest_digest(manifest: &CriteriaManifest) -> Result<String> {
    Ok(verification::digest(&serde_json::to_vec(manifest)?))
}

/// Register the reviewed mapping explicitly; spec edits never reconcile it automatically.
/// Also opts an existing current run into exactly this contract.
pub fn register_criteria(ctx: &Ctx, id: &str, manifest: &CriteriaManifest) -> Result<Task> {
    if manifest.version != 1 || manifest.reviewed_by.trim().is_empty() {
        bail!("invalid manifest version or missing owner review attribution");
    }
    let mut ids = HashSet::new();
    for criterion in &manifest.criteria {
        if !valid_key(&criterion.id)
            || !ids.insert(&criterion.id)
            || criterion.scenario.trim().is_empty()
            || criterion.expected.trim().is_empty()
        {
            bail!("invalid or duplicate criterion {}", criterion.id);
        }
    }
    if manifest.owner_decisions.iter().any(|d| d.trim().is_empty()) {
        bail!("owner decisions must be nonempty");
    }
    let task = ctx.store.update(id, |task| {
        if manifest.spec_digest != verification::spec_digest(task)? {
            bail!("manifest_spec_stale");
        }
        task.completion = Some(manifest.clone());
        let checks = match task.current_run() {
            Some(run) => {
                crate::gate_tier::select(
                    ctx,
                    task,
                    &corral_core::RunHead::of(run)?,
                    GatePhase::Review,
                )?
                .commands
            }
            None => verification::policy(ctx, task)?.gate,
        };
        if let Some(run) = task.current_run_mut() {
            run.completion.manifest = Some(manifest.clone());
            run.completion.required_checks = Some(checks);
            run.completion.audit = None;
        }
        ctx.store.log_event(
            Some(&task.id),
            "criteria_registered",
            json!({"manifest": manifest_digest(manifest)?, "reviewed_by": manifest.reviewed_by}),
        )?;
        Ok(())
    })?;
    Ok(task)
}

fn artifact_hashes<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    refs: &[String],
) -> Result<Vec<String>> {
    admitted_hashes(ctx, task, run, attempt_start(run), refs)
}

fn admitted_hashes<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    earliest: Timestamp,
    refs: &[String],
) -> Result<Vec<String>> {
    if refs.is_empty() {
        bail!("missing_artifact");
    }
    snapshot_hashes(ctx, task, &run.id, earliest, refs)
}

/// Bounded, confined byte validation of snapshots captured for `run` no earlier than `earliest`.
pub(crate) fn snapshot_hashes<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &str,
    earliest: Timestamp,
    refs: &[String],
) -> Result<Vec<String>> {
    let artifacts = task.artifacts_of(run)?;
    snapshot_hashes_with(ctx, task.id(), run, earliest, refs, &|path| {
        Ok(artifacts.iter().find(|a| a.path == path).cloned())
    })
}

/// [`snapshot_hashes`] with each referenced artifact looked up by its stored path.
pub(crate) fn snapshot_hashes_with(
    ctx: &Ctx,
    task: &str,
    run: &str,
    earliest: Timestamp,
    refs: &[String],
    artifact: &dyn Fn(&str) -> Result<Option<corral_core::Artifact>>,
) -> Result<Vec<String>> {
    let relative = Path::new("artifacts").join(task).join(run);
    let allowed = ctx.store.dir().join(&relative);
    let mut hashes = Vec::new();
    let mut seen = HashSet::new();
    for reference in refs {
        if !seen.insert(reference)
            || !Path::new(reference)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            bail!("invalid_artifact_path: {reference}");
        }
        let artifact = artifact(reference)?
            .filter(|a| a.run == run && a.purged_at.is_none())
            .with_context(|| format!("unknown_or_wrong_run_artifact: {reference}"))?;
        if artifact.added < earliest {
            bail!("artifact_before_attempt: {reference}");
        }
        if !Path::new(reference).starts_with(&relative) {
            bail!("artifact_path_escape: {reference}");
        }
        let path = ctx.store.dir().join(reference);
        let max = ctx.store.config.artifacts.max_bytes;
        if artifact.bytes == 0 || artifact.bytes > max {
            bail!("invalid_artifact_size: {reference}");
        }
        // Imported copies retain their original confinement checks. Canonical blobs need
        // neither a legacy directory nor a materialized snapshot to validate their bytes.
        if path.try_exists().context("inspecting legacy artifact")? {
            let root = ctx
                .store
                .dir()
                .canonicalize()
                .context("resolving store directory")?;
            let canonical_allowed = allowed
                .canonicalize()
                .context("resolving artifact directory")?;
            if canonical_allowed != root.join(&relative) {
                bail!("artifact_directory_escape");
            }
            let canonical = path
                .canonicalize()
                .with_context(|| format!("missing_artifact: {reference}"))?;
            if !canonical.starts_with(&canonical_allowed) {
                bail!("artifact_path_escape: {reference}");
            }
            let metadata = std::fs::metadata(&canonical).context("reading artifact metadata")?;
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() > max {
                bail!("invalid_artifact_size: {reference}");
            }
        }
        let bytes = crate::artifact_storage::artifact_read_confined_limited(ctx, &artifact, max)
            .map_err(|error| {
                // Canonical verification may reject corruption before returning decoded bytes.
                // Preserve the same evidence-boundary diagnoses as the explicit checks below.
                let kind = error.chain().find_map(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .map(std::io::Error::kind)
                });
                let label = match kind {
                    Some(std::io::ErrorKind::NotFound) => "missing_artifact",
                    Some(std::io::ErrorKind::InvalidData) => "artifact_tampered",
                    _ => "reading artifact",
                };
                error.context(format!("{label}: {reference}"))
            })?;
        let hash = verification::digest(&bytes);
        if u64::try_from(bytes.len())? != artifact.bytes || hash != artifact.sha256 {
            bail!("artifact_tampered: {reference}");
        }
        hashes.push(hash);
    }
    Ok(hashes)
}

pub(crate) fn current_identity<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
) -> Result<CandidateIdentity> {
    let receipt = run
        .gate
        .as_ref()
        .and_then(|g| g.identity.as_ref())
        .or_else(|| run.completion.evidence.first().map(|e| &e.identity));
    match receipt {
        Some(receipt) => verification::identity(ctx, task, run, receipt),
        None => verification::capture_identity(ctx, task, run, "no-gate"),
    }
}

pub(crate) fn manifest_failures<T: TaskRef + ?Sized>(
    task: &T,
    run: &RunHead,
) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    let Some(manifest) = &task.completion() else {
        return Ok(failures);
    };
    if manifest.version != 1 || manifest.reviewed_by.trim().is_empty() {
        failures.push("manifest_invalid".into());
    }
    if manifest.spec_digest != verification::spec_digest(task)? {
        failures.push("manifest_spec_stale".into());
    }
    if run.completion.manifest.as_ref() != Some(manifest) {
        failures.push("run_manifest_stale".into());
    }
    Ok(failures)
}

/// Append one assertion under the task lock. Worker input has no machine receipt field.
pub fn record_evidence(
    ctx: &Ctx,
    id: &str,
    observation: &EvidenceObservation,
) -> Result<EvidenceRecord> {
    evidence_inner(ctx, id, observation, None).map(|(record, _)| record)
}

/// The corral-pi `evidence` op: its receipt commits with the ledger entry.
pub fn record_evidence_request(
    ctx: &Ctx,
    id: &str,
    observation: &EvidenceObservation,
    origin: &RequestOrigin,
) -> Result<Receipted> {
    effects::swept(ctx, origin, effects::Home::of(origin), || {
        evidence_inner(ctx, id, observation, Some(origin)).map(|(_, receipt)| receipt)
    })
}

fn evidence_inner(
    ctx: &Ctx,
    id: &str,
    observation: &EvidenceObservation,
    origin: Option<&RequestOrigin>,
) -> Result<(EvidenceRecord, Option<RequestReceipt>)> {
    if observation.version != 1
        || !valid_key(&observation.key)
        || !valid_key(&observation.attempt)
        || [
            &observation.action,
            &observation.environment,
            &observation.expected,
            &observation.observed,
            &observation.producer,
        ]
        .iter()
        .any(|s| s.trim().is_empty())
        || observation
            .cleanup
            .as_ref()
            .is_some_and(|s| s.trim().is_empty())
    {
        bail!("invalid_evidence");
    }
    let mut result = None;
    let mut receipt = None;
    ctx.store.update(id, |task| {
        if let Some(origin) = origin {
            effects::recheck(&ctx.store, task, origin, &effects::Home::of(origin))?;
        }
        let run = task.current_run().context("no_current_run")?;
        let manifest = task.completion.as_ref().context("missing_manifest")?;
        let criterion = manifest
            .criteria
            .iter()
            .find(|c| c.id == observation.criterion)
            .context("unknown_criterion")?;
        if observation.run != run.id {
            bail!("wrong_run");
        }
        let failures = manifest_failures(task, &corral_core::RunHead::of(run)?)?;
        if !failures.is_empty() {
            bail!("{}", failures.join(", "));
        }
        if observation.expected != criterion.expected {
            bail!("criterion_expected_mismatch");
        }
        let hashes = artifact_hashes(
            ctx,
            task,
            &corral_core::RunHead::of(run)?,
            &observation.artifacts,
        )?;
        if let Some(existing) = run
            .completion
            .evidence
            .iter()
            .find(|e| e.observation.key == observation.key)
        {
            if existing.observation != *observation {
                bail!("conflicting_idempotency_key");
            }
            result = Some(existing.clone());
            if let Some(origin) = origin {
                receipt = Some(evidence_receipt(task, origin, &observation.key, None)?);
            }
            return Ok(());
        }
        let identity = current_identity(ctx, task, &corral_core::RunHead::of(run)?)?;
        if identity.attempt != observation.attempt {
            bail!("wrong_attempt");
        }
        if let Some(gate) = &run.gate {
            if gate.result == GateResult::Running {
                bail!("gate_incomplete");
            }
            if !review_gate_fresh(ctx, task, &corral_core::RunHead::of(run)?, gate)? {
                bail!("gate_stale");
            }
        } else if manifest.require_gate || !verification::policy(ctx, task)?.gate.is_empty() {
            bail!("missing_gate_identity");
        }
        check_capture_window(
            observation.captured,
            &corral_core::RunHead::of(run)?,
            corral_core::clock::now(),
        )?;
        let record = EvidenceRecord {
            source: EvidenceSource::WorkerAssertion,
            observation: observation.clone(),
            identity,
            manifest_digest: manifest_digest(manifest)?,
            artifact_hashes: hashes,
        };
        task.current_run_mut()
            .context("no_current_run")?
            .completion
            .evidence
            .push(record.clone());
        if let Some(origin) = origin {
            let detail = evidence_detail(&record);
            receipt = Some(evidence_receipt(
                task,
                origin,
                &observation.key,
                Some(detail),
            )?);
        }
        if origin.is_none() {
            ctx.store.log_event(
                Some(&task.id),
                "evidence_recorded",
                evidence_detail(&record),
            )?;
        }
        result = Some(record);
        Ok(())
    })?;
    let record = result.context("evidence update produced no result")?;
    Ok((record, receipt))
}

fn evidence_detail(record: &EvidenceRecord) -> serde_json::Value {
    json!({
        "run": record.identity.run, "attempt": record.identity.attempt,
        "key": record.observation.key, "criterion": record.observation.criterion,
        "source": "worker_assertion",
    })
}

fn evidence_receipt(
    task: &mut Task,
    origin: &RequestOrigin,
    key: &str,
    appended: Option<serde_json::Value>,
) -> Result<RequestReceipt> {
    let mut result = json!({"key": key, "appended": appended.is_some()});
    if let Some(detail) = appended {
        result["event"] = effects::event("evidence_recorded", detail, origin);
    }
    let owed = if result.get("event").is_some() {
        vec!["effect:event"]
    } else {
        Vec::new()
    };
    let commit = effects::Commit {
        result,
        effect: Some(("evidence", key.to_owned())),
        owed,
    };
    effects::record(task, origin, &effects::Home::of(origin), commit)
}

/// Read the canonical ledger; freshness is reported separately by the completion audit.
pub fn evidence_list(ctx: &Ctx, id: &str) -> Result<Vec<EvidenceRecord>> {
    let task = ctx.store.get(id)?;
    Ok(task
        .current_run()
        .context("no_current_run")?
        .completion
        .evidence
        .clone())
}

/// Required evidence presence is not semantic acceptance. Latest observations supersede
/// earlier assertions, but never mutate them or promote them to machine PASS.
pub fn evidence_failures<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
) -> Result<Vec<String>> {
    let Some(manifest) = &task.completion() else {
        return Ok(Vec::new());
    };
    let complete = |id: &str| {
        run.progress
            .as_ref()
            .is_some_and(|p| p.criterion_complete(id))
    };
    if !manifest.audited && !manifest.criteria.iter().any(|c| complete(&c.id)) {
        return Ok(Vec::new());
    }
    let mut failures = manifest_failures(task, run)?;
    if manifest.audited {
        let plan_notes = crate::plan::accepted_owner_notes(ctx, task)?;
        for decision in &manifest.owner_decisions {
            let resolved = plan_notes.as_ref().is_some_and(|notes| {
                notes.lines().any(|line| {
                    line.strip_prefix("Resolved: ")
                        .and_then(|line| line.split_once(" => "))
                        .is_some_and(|(question, answer)| {
                            question.trim() == decision.trim() && !answer.trim().is_empty()
                        })
                })
            });
            if !resolved {
                failures.push(format!("owner_decision:{decision}"));
            }
        }
        for decision in task
            .open_obligations(&run.id)?
            .decisions
            .iter()
            .filter(|d| d.pending())
        {
            failures.push(format!("owner_decision:{}", decision.request.id));
        }
    }
    let identity = current_identity(ctx, task, run)?;
    let current = Admission {
        identity: &identity,
        start: attempt_start(run),
        end: None,
    };
    let carry = evidence_carry::live(task, run, &identity)?;
    let carried = carry.map(|c| Admission {
        identity: &c.from,
        start: c.window_start,
        end: Some(c.at),
    });
    failures.extend(obligation_failures(
        ctx,
        task,
        run,
        &current,
        carried.as_ref(),
    )?);
    Ok(failures)
}

/// The identity a criterion record must be bound to, and its capture and artifact window.
pub(crate) struct Admission<'a> {
    pub identity: &'a CandidateIdentity,
    pub start: Timestamp,
    pub end: Option<Timestamp>,
}

/// Identity-dependent checks of every evidence obligation: each required criterion of an
/// audited manifest and each criterion progress marks complete. A record bound to the
/// `carried` identity is admitted under that window; any other record is judged by `current`.
pub(crate) fn obligation_failures<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    current: &Admission,
    carried: Option<&Admission>,
) -> Result<Vec<String>> {
    inspection_failures(ctx, task, run, current, carried, false)
}

/// The same admission checks for inspection, optionally including non-obligatory criteria.
/// This does not change completion policy or promote worker assertions to machine receipts.
pub(crate) fn inspection_failures<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    current: &Admission,
    carried: Option<&Admission>,
    all: bool,
) -> Result<Vec<String>> {
    let Some(manifest) = &task.completion() else {
        return Ok(Vec::new());
    };
    let complete = |id: &str| {
        run.progress
            .as_ref()
            .is_some_and(|p| p.criterion_complete(id))
    };
    let mut failures = Vec::new();
    let digest = manifest_digest(manifest)?;
    for criterion in manifest
        .criteria
        .iter()
        .filter(|c| all || (manifest.audited && c.required) || complete(&c.id))
    {
        let before = failures.len();
        let Some(record) = run
            .completion
            .evidence
            .iter()
            .rev()
            .find(|e| e.observation.criterion == criterion.id)
        else {
            failures.push(format!("evidence_missing:{}", criterion.id));
            if complete(&criterion.id) {
                failures.push(format!("progress_without_evidence:{}", criterion.id));
            }
            continue;
        };
        let name = &criterion.id;
        let admission = match carried {
            Some(c) if record.identity != *current.identity && record.identity == *c.identity => c,
            _ => current,
        };
        if record.identity != *admission.identity || record.manifest_digest != digest {
            failures.push(format!("evidence_stale:{name}"));
        }
        let captured = record.observation.captured;
        if captured < admission.start || admission.end.is_some_and(|end| captured > end) {
            failures.push(format!("evidence_capture_stale:{name}"));
        }
        match record.observation.status {
            ObservationStatus::Pass => {}
            ObservationStatus::Fail => failures.push(format!("evidence_failed:{name}")),
            ObservationStatus::NotRun => failures.push(format!("evidence_not_run:{name}")),
        }
        if criterion.live && !record.observation.live {
            failures.push(format!("evidence_not_live:{name}"));
        }
        let refs = &record.observation.artifacts;
        match admitted_hashes(ctx, task, run, admission.start, refs) {
            Ok(hashes) if hashes == record.artifact_hashes => {}
            Ok(_) | Err(_) => failures.push(format!("evidence_artifact_invalid:{name}")),
        }
        if complete(name) && failures.len() > before {
            failures.push(format!("progress_without_evidence:{name}"));
        }
    }
    Ok(failures)
}

/// The worker's receipt is judged by the review-phase contract, also once an approval exists.
fn review_gate_fresh<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    gate: &GateRecord,
) -> Result<bool> {
    let requirement = crate::gate_tier::requirement_for(ctx, task, run, GatePhase::Review)?;
    verification::gate_fresh_for(ctx, task, run, gate, &requirement)
}

/// Bounds compare stamps from `corral_core::clock`, which never runs backwards in-process.
/// The attempt starts at the gate stamp when a gate exists, else at the run start.
fn predates_attempt(stamp: Timestamp, run: &RunHead) -> bool {
    stamp < attempt_start(run)
}

pub(crate) fn attempt_start(run: &RunHead) -> Timestamp {
    run.gate.as_ref().map_or(run.started, |gate| gate.at)
}

fn check_capture_window(captured: Timestamp, run: &RunHead, now: Timestamp) -> Result<()> {
    if predates_attempt(captured, run) || captured > now {
        let earliest = run.gate.as_ref().map_or(run.started, |gate| gate.at);
        bail!(
            "capture_outside_attempt: possible cross-process clock rollback or out-of-window capture; captured={captured} earliest={earliest} now={now}"
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "completion_clock_tests.rs"]
mod clock_tests;

fn git_output(run: &RunHead, args: &[&str]) -> Result<std::process::Output> {
    corral_git::work::record();
    Command::new("git")
        .arg("-C")
        .arg(&run.worktree)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running git {args:?} for completion audit"))
}

/// Lock-safe read-only variant for `Store::update`: uses its current task, never re-locks it.
pub fn completion_check_task<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    summary: &str,
) -> Result<CompletionAudit> {
    let audited = task.completion().as_ref().is_some_and(|m| m.audited);
    let mut audit = CompletionAudit {
        version: 1,
        audited,
        passed: true,
        identity: None,
        manifest_digest: None,
        required_checks: Vec::new(),
        evidence_keys: Vec::new(),
        failures: Vec::new(),
        checked: corral_core::clock::now(),
    };
    let Some(run) = task.current_run_head()? else {
        audit.failures.push("no_current_run".into());
        audit.passed = false;
        return Ok(audit);
    };
    audit.failures.extend(evidence_failures(ctx, task, &run)?);
    if !audited {
        audit.identity = Some(current_identity(ctx, task, &run)?);
        audit.passed = audit.failures.is_empty();
        return Ok(audit);
    }
    let manifest = task.completion().as_ref().context("missing_manifest")?;
    audit.manifest_digest = Some(manifest_digest(manifest)?);
    audit.required_checks = crate::gate_tier::select(ctx, task, &run, GatePhase::Review)?.commands;
    if run.completion.required_checks.as_ref() != Some(&audit.required_checks) {
        audit.failures.push("completion_policy_changed".into());
    }
    for section in ["Verified", "Not verified"] {
        let present = summary.lines().any(|line| {
            let line = line.trim().trim_start_matches('#').trim();
            let line = line.trim_start_matches("**");
            line.strip_prefix(section).is_some_and(|tail| {
                tail.starts_with(':') || tail.starts_with("**:") || tail.is_empty()
            })
        });
        if !present {
            audit
                .failures
                .push(format!("summary_section_missing:{section}"));
        }
    }
    if !corral_git::is_clean(&run.worktree)? {
        audit.failures.push("worktree_dirty".into());
    }
    for operation in [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "sequencer",
        "BISECT_LOG",
    ] {
        let out = git_output(&run, &["rev-parse", "--git-path", operation])?;
        if !out.status.success() {
            bail!("cannot inspect git operation state");
        }
        let path = String::from_utf8(out.stdout).context("decoding git operation path")?;
        if run.worktree.join(path.trim()).exists() {
            audit.failures.push(format!("git_operation:{operation}"));
        }
    }
    let identity = current_identity(ctx, task, &run)?;
    let ancestry = git_output(
        &run,
        &[
            "merge-base",
            "--is-ancestor",
            &identity.integration_base,
            &identity.head,
        ],
    )?;
    match ancestry.status.code() {
        Some(0) => {}
        Some(1) => audit.failures.push("integration_base_not_ancestor".into()),
        _ => bail!("cannot inspect integration base ancestry"),
    }
    if manifest.require_gate || !audit.required_checks.is_empty() {
        match &run.gate {
            None => audit.failures.push("gate_missing".into()),
            Some(gate) => {
                if gate.result != GateResult::Pass {
                    audit.failures.push("gate_not_passed".into());
                }
                if !review_gate_fresh(ctx, task, &run, gate)? {
                    audit.failures.push("gate_stale".into());
                }
            }
        }
    }
    let carried = evidence_carry::live(task, &run, &identity)?.map(|c| &c.from);
    audit.evidence_keys = run
        .completion
        .evidence
        .iter()
        .filter(|e| e.identity == identity || carried == Some(&e.identity))
        .map(|e| e.observation.key.clone())
        .collect();
    audit.identity = Some(identity);
    audit.passed = audit.failures.is_empty();
    Ok(audit)
}

/// Preflight only: no commits, snapshots, events, or status changes.
pub fn completion_check(ctx: &Ctx, id: &str, summary: &str) -> Result<CompletionAudit> {
    completion_check_task(ctx, &ctx.store.head(id)?, summary)
}
