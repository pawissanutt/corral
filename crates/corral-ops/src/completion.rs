//! Criterion registration, assertion ledger, and read-only completion intake.
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path};
use std::process::Command;

use anyhow::bail;
use anyhow::{Context, Result};
use corral_core::{
    CandidateIdentity, CompletionAudit, CriteriaManifest, EvidenceObservation, EvidenceRecord,
    EvidenceSource, GateResult, ObservationStatus, Run, Task,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

use crate::verification;

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

fn manifest_digest(manifest: &CriteriaManifest) -> Result<String> {
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
        let checks = verification::policy(ctx, task)?.gate;
        if let Some(run) = task.current_run_mut() {
            run.completion.manifest = Some(manifest.clone());
            run.completion.required_checks = Some(checks);
            run.completion.audit = None;
        }
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "criteria_registered",
        json!({"manifest": manifest_digest(manifest)?, "reviewed_by": manifest.reviewed_by}),
    )?;
    Ok(task)
}

fn artifact_hashes(ctx: &Ctx, task: &Task, run: &Run, refs: &[String]) -> Result<Vec<String>> {
    if refs.is_empty() {
        bail!("missing_artifact");
    }
    let allowed = ctx.store.artifacts_dir(&task.id, &run.id);
    let allowed = allowed
        .canonicalize()
        .context("missing artifact snapshot directory")?;
    let root = ctx
        .store
        .dir()
        .canonicalize()
        .context("resolving store directory")?;
    if allowed != root.join("artifacts").join(&task.id).join(&run.id) {
        bail!("artifact_directory_escape");
    }
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
        let artifact = task
            .artifacts
            .iter()
            .find(|a| a.run == run.id && a.path == *reference)
            .with_context(|| format!("unknown_or_wrong_run_artifact: {reference}"))?;
        if predates_attempt(artifact.added, run) {
            bail!("artifact_before_attempt: {reference}");
        }
        let path = ctx
            .store
            .dir()
            .join(reference)
            .canonicalize()
            .with_context(|| format!("missing_artifact: {reference}"))?;
        if !path.starts_with(&allowed) {
            bail!("artifact_path_escape: {reference}");
        }
        let metadata = std::fs::metadata(&path).context("reading artifact metadata")?;
        let max = ctx.store.config.artifacts.max_bytes;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > max {
            bail!("invalid_artifact_size: {reference}");
        }
        let file =
            std::fs::File::open(&path).with_context(|| format!("opening artifact {reference}"))?;
        let mut bytes = Vec::new();
        file.take(max.saturating_add(1))
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading artifact {reference}"))?;
        let hash = verification::digest(&bytes);
        if u64::try_from(bytes.len())? != artifact.bytes || hash != artifact.sha256 {
            bail!("artifact_tampered: {reference}");
        }
        hashes.push(hash);
    }
    Ok(hashes)
}

fn current_identity(ctx: &Ctx, task: &Task, run: &Run) -> Result<CandidateIdentity> {
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

fn manifest_failures(task: &Task, run: &Run) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    let Some(manifest) = &task.completion else {
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
    let mut appended = false;
    let task = ctx.store.update(id, |task| {
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
        let failures = manifest_failures(task, run)?;
        if !failures.is_empty() {
            bail!("{}", failures.join(", "));
        }
        if observation.expected != criterion.expected {
            bail!("criterion_expected_mismatch");
        }
        let hashes = artifact_hashes(ctx, task, run, &observation.artifacts)?;
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
            return Ok(());
        }
        let identity = current_identity(ctx, task, run)?;
        if identity.attempt != observation.attempt {
            bail!("wrong_attempt");
        }
        if let Some(gate) = &run.gate {
            if gate.result == GateResult::Running {
                bail!("gate_incomplete");
            }
            if !verification::gate_fresh(ctx, task, run, gate)? {
                bail!("gate_stale");
            }
        } else if manifest.require_gate || !verification::policy(ctx, task)?.gate.is_empty() {
            bail!("missing_gate_identity");
        }
        check_capture_window(observation.captured, run, corral_core::clock::now())?;
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
        result = Some(record);
        appended = true;
        Ok(())
    })?;
    let record = result.context("evidence update produced no result")?;
    if appended {
        ctx.store.log_event(
            Some(&task.id),
            "evidence_recorded",
            json!({
                "run": record.identity.run, "attempt": record.identity.attempt,
                "key": record.observation.key, "criterion": record.observation.criterion,
                "source": "worker_assertion",
            }),
        )?;
    }
    Ok(record)
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
pub fn evidence_failures(ctx: &Ctx, task: &Task, run: &Run) -> Result<Vec<String>> {
    let Some(manifest) = &task.completion else {
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
        for decision in run.obligations.decisions.iter().filter(|d| d.pending()) {
            failures.push(format!("owner_decision:{}", decision.request.id));
        }
    }
    let identity = current_identity(ctx, task, run)?;
    let digest = manifest_digest(manifest)?;
    for criterion in manifest
        .criteria
        .iter()
        .filter(|c| (manifest.audited && c.required) || complete(&c.id))
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
        if record.identity != identity || record.manifest_digest != digest {
            failures.push(format!("evidence_stale:{name}"));
        }
        if predates_attempt(record.observation.captured, run) {
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
        match artifact_hashes(ctx, task, run, &record.observation.artifacts) {
            Ok(hashes) if hashes == record.artifact_hashes => {}
            Ok(_) | Err(_) => failures.push(format!("evidence_artifact_invalid:{name}")),
        }
        if complete(name) && failures.len() > before {
            failures.push(format!("progress_without_evidence:{name}"));
        }
    }
    Ok(failures)
}

/// Bounds compare stamps from `corral_core::clock`, which never runs backwards in-process.
/// The attempt starts at the gate stamp when a gate exists, else at the run start.
fn predates_attempt(stamp: Timestamp, run: &Run) -> bool {
    stamp < run.gate.as_ref().map_or(run.started, |gate| gate.at)
}

fn check_capture_window(captured: Timestamp, run: &Run, now: Timestamp) -> Result<()> {
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

fn git_output(run: &Run, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(&run.worktree)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("running git {args:?} for completion audit"))
}

/// Lock-safe read-only variant for `Store::update`: uses its current task, never re-locks it.
pub fn completion_check_task(ctx: &Ctx, task: &Task, summary: &str) -> Result<CompletionAudit> {
    let audited = task.completion.as_ref().is_some_and(|m| m.audited);
    let mut audit = CompletionAudit {
        version: 1,
        audited,
        passed: true,
        identity: None,
        manifest_digest: None,
        required_checks: Vec::new(),
        evidence_keys: Vec::new(),
        failures: Vec::new(),
        checked: Timestamp::now(),
    };
    let Some(run) = task.current_run() else {
        audit.failures.push("no_current_run".into());
        audit.passed = false;
        return Ok(audit);
    };
    audit.failures.extend(evidence_failures(ctx, task, run)?);
    if !audited {
        audit.identity = Some(current_identity(ctx, task, run)?);
        audit.passed = audit.failures.is_empty();
        return Ok(audit);
    }
    let manifest = task.completion.as_ref().context("missing_manifest")?;
    audit.manifest_digest = Some(manifest_digest(manifest)?);
    audit.required_checks = verification::policy(ctx, task)?.gate;
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
        let out = git_output(run, &["rev-parse", "--git-path", operation])?;
        if !out.status.success() {
            bail!("cannot inspect git operation state");
        }
        let path = String::from_utf8(out.stdout).context("decoding git operation path")?;
        if run.worktree.join(path.trim()).exists() {
            audit.failures.push(format!("git_operation:{operation}"));
        }
    }
    let identity = current_identity(ctx, task, run)?;
    let ancestry = git_output(
        run,
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
                if !verification::gate_fresh(ctx, task, run, gate)? {
                    audit.failures.push("gate_stale".into());
                }
            }
        }
    }
    audit.evidence_keys = run
        .completion
        .evidence
        .iter()
        .filter(|e| e.identity == identity)
        .map(|e| e.observation.key.clone())
        .collect();
    audit.identity = Some(identity);
    audit.passed = audit.failures.is_empty();
    Ok(audit)
}

/// Preflight only: no commits, snapshots, events, or status changes.
pub fn completion_check(ctx: &Ctx, id: &str, summary: &str) -> Result<CompletionAudit> {
    completion_check_task(ctx, &ctx.store.get(id)?, summary)
}
