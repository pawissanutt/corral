//! Guarded progress writes and immutable, portable checkpoint snapshots.
use std::collections::HashSet;
use std::io::Read;

use anyhow::{Context, Result, bail};
use corral_core::{
    Artifact, ArtifactKind, Checkpoint, CheckpointRecord, CheckpointUpdate, ProgressReadback,
    ProgressUpdate, Run, RunProgress, Status, StepState, Task,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::Ctx;
use crate::agent::effects::{self, Receipted};
use corral_core::agent::{RequestOrigin, RequestReceipt};

pub fn progress_spec_revision(task: &Task) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &task.id,
            &task.title,
            &task.project,
            &task.body,
            &task.completion,
        ))?)
    ))
}

pub(crate) fn tree_revision(run: &Run) -> Result<String> {
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let mut digest = Sha256::new();
    digest.update(head.as_bytes());
    // Diff metadata includes executable-bit and other git-level changes.
    digest.update(corral_git::diff_text(&run.worktree, "HEAD")?.as_bytes());
    let mut changes = corral_git::diff_stat(&run.worktree, "HEAD")?.files;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    for change in changes {
        digest.update(serde_json::to_vec(&change.path)?);
        let path = run.worktree.join(&change.path);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                digest.update(b"deleted");
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if meta.is_symlink() {
            digest.update(b"symlink");
            digest.update(
                std::fs::read_link(&path)
                    .with_context(|| format!("reading link {}", path.display()))?
                    .as_os_str()
                    .as_encoded_bytes(),
            );
        } else if meta.is_file() {
            digest.update(b"file");
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("opening {}", path.display()))?;
            let mut content = Sha256::new();
            let mut buf = [0u8; 8192];
            loop {
                let len = file
                    .read(&mut buf)
                    .with_context(|| format!("hashing {}", path.display()))?;
                if len == 0 {
                    break;
                }
                content.update(&buf[..len]);
            }
            digest.update(content.finalize());
        } else {
            bail!("unsupported_worktree_file: {}", path.display());
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn progress_readback(ctx: &Ctx, id: &str) -> Result<ProgressReadback> {
    // One locked snapshot: metadata must not combine two different run revisions.
    ctx.store.read_locked(id, |task| {
        let run = task.current_run().context("missing_run")?;
        let packet = crate::resume_packet(ctx, task, run)?;
        Ok(ProgressReadback {
            task: task.id.clone(),
            run: run.id.clone(),
            spec_revision: progress_spec_revision(task)?,
            head: corral_git::rev_sha(&run.worktree, "HEAD")?,
            base: corral_git::rev_sha(&run.worktree, &run.base)?,
            tree: tree_revision(run)?,
            progress: run.progress.clone(),
            criteria: corral_core::criterion_progress(task, run, run.progress.as_ref()),
            evidence_failures: crate::evidence_failures(ctx, task, run)?,
            checkpoints: run.checkpoints.clone(),
            checkpoint_status: packet.status,
            checkpoint_reasons: packet.reasons,
        })
    })
}

fn active_run<'a>(task: &'a Task, run_id: &str, spec: &str, handoff: bool) -> Result<&'a Run> {
    let run = task.current_run().context("missing_run")?;
    if run.id != run_id {
        bail!("stale_run: expected {}, received {run_id}", run.id);
    }
    let active = matches!(task.status, Status::Running | Status::NeedsMe) && run.outcome.is_none();
    let finished_handoff =
        handoff && task.awaiting_owner() && run.outcome == Some(corral_core::RunOutcome::Done);
    if !active && !finished_handoff {
        bail!("run_closed: {}", run.id);
    }
    if progress_spec_revision(task)? != spec {
        bail!("stale_spec");
    }
    Ok(run)
}

fn identity(task: &Task, input_task: &str) -> Result<()> {
    if task.id != input_task {
        bail!("task_identity_mismatch");
    }
    Ok(())
}

fn unique<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut seen = HashSet::new();
    for id in ids {
        if id.trim().is_empty() || !seen.insert(id) {
            bail!("duplicate_or_empty_id: {id}");
        }
    }
    Ok(())
}

/// Resolve links through the canonical acceptance manifest and current run's receipts.
fn validate_references(task: &Task, criteria: &[String], evidence: &[String]) -> Result<()> {
    unique(criteria.iter().map(String::as_str))?;
    unique(evidence.iter().map(String::as_str))?;
    for id in criteria {
        if !task
            .completion
            .as_ref()
            .is_some_and(|m| m.criteria.iter().any(|c| &c.id == id))
        {
            bail!("unknown_criterion: {id}");
        }
    }
    for reference in evidence {
        let record = task
            .current_run()
            .and_then(|r| {
                r.completion
                    .evidence
                    .iter()
                    .find(|e| &e.observation.key == reference && e.observation.run == r.id)
            })
            .with_context(|| format!("unknown_evidence_reference: {reference}"))?;
        if !criteria.contains(&record.observation.criterion) {
            bail!("evidence_criterion_mismatch: {reference}");
        }
    }
    Ok(())
}

pub fn update_progress(ctx: &Ctx, id: &str, input: &ProgressUpdate) -> Result<Task> {
    progress_inner(ctx, id, input, None).map(|(task, _)| task)
}

/// The Pi-native `progress` op: its receipt commits in the progress write.
pub fn update_progress_request(
    ctx: &Ctx,
    id: &str,
    input: &ProgressUpdate,
    origin: &RequestOrigin,
) -> Result<Receipted> {
    effects::swept(ctx, origin, effects::Home::of(origin), || {
        progress_inner(ctx, id, input, Some(origin)).map(|(_, receipt)| receipt)
    })
}

fn progress_inner(
    ctx: &Ctx,
    id: &str,
    input: &ProgressUpdate,
    origin: Option<&RequestOrigin>,
) -> Result<(Task, Option<RequestReceipt>)> {
    let detail = json!({"run": input.run, "revision": input.expected_revision + 1});
    let mut receipt = None;
    let task = ctx.store.update(id, |task| {
        if let Some(origin) = origin {
            effects::recheck(&ctx.store, task, origin, &effects::Home::of(origin))?;
        }
        identity(task, &input.task)?;
        let run = active_run(task, &input.run, &input.spec_revision, false)?;
        let revision = run.progress.as_ref().map_or(0, |p| p.revision);
        if revision != input.expected_revision {
            bail!("stale_progress: expected {revision}");
        }
        unique(input.phases.iter().map(|p| p.id.as_str()))?;
        let steps: Vec<_> = input.phases.iter().flat_map(|p| &p.steps).collect();
        unique(steps.iter().map(|s| s.id.as_str()))?;
        if input.phases.iter().any(|p| p.label.trim().is_empty())
            || steps.iter().any(|s| s.label.trim().is_empty())
        {
            bail!("empty_progress_label");
        }
        if steps
            .iter()
            .filter(|s| s.status == StepState::InProgress)
            .count()
            > 1
        {
            bail!("multiple_current_steps");
        }
        for step in &steps {
            validate_references(task, &step.criteria, &[])?;
            if let StepState::Dropped { reason } = &step.status
                && reason.trim().is_empty()
            {
                bail!("drop_reason_required: {}", step.id);
            }
        }
        if let Some(previous) = &run.progress {
            for phase in &previous.phases {
                if !input.phases.iter().any(|p| p.id == phase.id) {
                    bail!("phase_removed: {}; retain its identity", phase.id);
                }
            }
            for step in previous.steps() {
                let next = steps.iter().find(|s| s.id == step.id).with_context(|| {
                    format!("step_removed: {}; drop with a reason instead", step.id)
                })?;
                match &step.status {
                    StepState::Done | StepState::Dropped { .. } if next.status != step.status => {
                        bail!("terminal_step_changed: {}", step.id);
                    }
                    StepState::Pending
                    | StepState::InProgress
                    | StepState::Done
                    | StepState::Dropped { .. } => {}
                }
            }
        }
        let revision = revision
            .checked_add(1)
            .context("progress_revision_exhausted")?;
        let run = task.current_run_mut().context("missing_run")?;
        run.progress = Some(RunProgress {
            revision,
            spec_revision: input.spec_revision.clone(),
            phases: input.phases.clone(),
        });
        run.checkpoint_required = true;
        if let Some(origin) = origin {
            let event = effects::event("progress_updated", detail.clone(), origin);
            receipt = Some(effects::record(
                task,
                origin,
                &effects::Home::of(origin),
                effects::Commit {
                    result: json!({"run": input.run, "revision": revision, "event": event}),
                    effect: Some(("progress", revision.to_string())),
                    owed: vec!["effect:event"],
                },
            )?);
        }
        Ok(())
    })?;
    if origin.is_none() {
        ctx.store.log_event(Some(id), "progress_updated", detail)?;
    }
    Ok((task, receipt))
}

pub fn save_checkpoint(ctx: &Ctx, id: &str, input: &CheckpointUpdate) -> Result<Task> {
    checkpoint_inner(ctx, id, input, None).map(|(task, _)| task)
}

/// The Pi-native `checkpoint` op: its receipt commits with the checkpoint record.
pub fn save_checkpoint_request(
    ctx: &Ctx,
    id: &str,
    input: &CheckpointUpdate,
    origin: &RequestOrigin,
) -> Result<Receipted> {
    effects::swept(ctx, origin, effects::Home::of(origin), || {
        checkpoint_inner(ctx, id, input, Some(origin)).map(|(_, receipt)| receipt)
    })
}

fn checkpoint_inner(
    ctx: &Ctx,
    id: &str,
    input: &CheckpointUpdate,
    origin: Option<&RequestOrigin>,
) -> Result<(Task, Option<RequestReceipt>)> {
    let detail = json!({"run": input.run, "revision": input.expected_revision + 1});
    let mut receipt = None;
    let task =
        ctx.store.update(id, |task| {
            if let Some(origin) = origin {
                effects::recheck(&ctx.store, task, origin, &effects::Home::of(origin))?;
            }
            identity(task, &input.task)?;
            let run = active_run(task, &input.run, &input.spec_revision, true)?;
            let revision = run.checkpoints.last().map_or(0, |c| c.checkpoint.revision);
            if revision != input.expected_revision {
                bail!("stale_checkpoint: expected {revision}");
            }
            if run.progress.as_ref().map_or(0, |p| p.revision) != input.progress_revision {
                bail!("stale_progress");
            }
            if run
                .progress
                .as_ref()
                .is_some_and(|p| p.spec_revision != input.spec_revision)
            {
                bail!("stale_progress_spec");
            }
            if corral_git::rev_sha(&run.worktree, "HEAD")? != input.head {
                bail!("stale_head");
            }
            if corral_git::rev_sha(&run.worktree, &run.base)? != input.base {
                bail!("stale_base");
            }
            if tree_revision(run)? != input.tree {
                bail!("stale_tree");
            }
            if input.next_action.trim().is_empty() {
                bail!("next_action_required");
            }
            validate_references(task, &input.criterion_ids, &input.evidence_refs)?;
            unique(input.waits.iter().map(|w| w.id.as_str()))?;
            unique(input.resources.iter().map(|r| r.id.as_str()))?;
            if input
                .waits
                .iter()
                .any(|w| w.resolution.as_ref().is_some_and(|s| s.trim().is_empty()))
                || input
                    .resources
                    .iter()
                    .any(|r| r.resolution.as_ref().is_some_and(|s| s.trim().is_empty()))
            {
                bail!("empty_obligation_resolution");
            }
            if let Some(previous) = run.checkpoints.last() {
                let old = &previous.checkpoint.input;
                if old.waits.iter().any(|w| {
                    w.resolution.is_none() && !input.waits.iter().any(|new| new.id == w.id)
                }) || old.resources.iter().any(|r| {
                    r.resolution.is_none() && !input.resources.iter().any(|new| new.id == r.id)
                }) {
                    bail!("pending_obligation_removed: retain it or record an explicit resolution");
                }
            }
            if input.decisions.iter().any(|d| d.trim().is_empty())
                || input.waits.iter().any(|w| {
                    [&w.owner, &w.handle, &w.condition]
                        .iter()
                        .any(|v| v.trim().is_empty())
                })
                || input.resources.iter().any(|r| {
                    [&r.owner, &r.description, &r.cleanup]
                        .iter()
                        .any(|v| v.trim().is_empty())
                })
            {
                bail!("empty_obligation");
            }
            let mut checkpoint = Checkpoint {
                version: 1,
                revision: revision
                    .checked_add(1)
                    .context("checkpoint_revision_exhausted")?,
                input: input.clone(),
                progress: run.progress.clone(),
                run_obligations: run.obligations.clone(),
                completed_steps: run
                    .progress
                    .iter()
                    .flat_map(|p| p.steps())
                    .filter(|s| s.status == StepState::Done)
                    .map(|s| s.id.clone())
                    .collect(),
                current_step: run
                    .progress
                    .as_ref()
                    .and_then(|p| p.current_step())
                    .map(|s| s.id.clone()),
                captured: corral_core::clock::now(),
            };
            let dir = ctx.store.artifacts_dir(&task.id, &run.id);
            if origin.is_some()
                && let Some(earlier) = materialized(&dir, &checkpoint)?
            {
                checkpoint.captured = earlier.captured;
            }
            let bytes = serde_json::to_vec_pretty(&checkpoint).context("encoding checkpoint")?;
            if u64::try_from(bytes.len())? > ctx.store.config.artifacts.max_bytes {
                bail!("checkpoint_too_large");
            }
            let sha = format!("{:x}", Sha256::digest(&bytes));
            let name = format!("{}-checkpoint-{}.json", sha, checkpoint.revision);
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let dest = dir.join(&name);
            std::fs::write(&dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
            if origin.is_some() {
                effects::failpoint("swept_before_commit");
            }
            let path = format!("artifacts/{}/{}/{name}", task.id, run.id);
            let artifact = Artifact {
                tier_override: None,
                compressed_bytes: None,
                purged_at: None,
                run: run.id.clone(),
                kind: ArtifactKind::Evidence,
                title: format!("Resume checkpoint {}", checkpoint.revision),
                path: path.clone(),
                source: dest.display().to_string(),
                sha256: sha,
                bytes: u64::try_from(bytes.len())?,
                added: checkpoint.captured,
            };
            task.current_run_mut()
                .context("missing_run")?
                .checkpoints
                .push(CheckpointRecord {
                    checkpoint,
                    artifact: path,
                });
            task.current_run_mut()
                .context("missing_run")?
                .checkpoint_required = true;
            let saved = artifact.path.clone();
            task.artifacts.push(artifact);
            if let Some(origin) = origin {
                let event = effects::event("checkpoint_saved", detail.clone(), origin);
                let commit = effects::Commit {
                    result: json!({
                        "run": input.run, "revision": revision + 1,
                        "artifact": saved, "event": event,
                    }),
                    effect: Some(("checkpoint", (revision + 1).to_string())),
                    owed: vec!["effect:event"],
                };
                let home = effects::Home::of(origin);
                receipt = Some(effects::record(task, origin, &home, commit)?);
            }
            Ok(())
        })?;
    if origin.is_none() {
        ctx.store.log_event(Some(id), "checkpoint_saved", detail)?;
    }
    Ok((task, receipt))
}

/// The blob an interrupted attempt already wrote for this revision, identical apart from its
/// capture time. A request's replay reuses that time, so its checkpoint stays content-addressed
/// and the pre-commit blob is not materialized twice.
fn materialized(dir: &std::path::Path, checkpoint: &Checkpoint) -> Result<Option<Checkpoint>> {
    let suffix = format!("-checkpoint-{}.json", checkpoint.revision);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("listing {}", dir.display())),
    };
    for entry in entries {
        let path = entry
            .with_context(|| format!("listing {}", dir.display()))?
            .path();
        let named = path.file_name().and_then(|n| n.to_str());
        if !named.is_some_and(|n| n.ends_with(&suffix)) {
            continue;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let Ok(earlier) = serde_json::from_slice::<Checkpoint>(&bytes) else {
            continue;
        };
        let retimed = Checkpoint {
            captured: checkpoint.captured,
            ..earlier.clone()
        };
        if retimed == *checkpoint {
            return Ok(Some(earlier));
        }
    }
    Ok(None)
}
