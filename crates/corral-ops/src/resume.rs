//! Resume packets are structured data, never reconstructed from terminal prose.
use anyhow::{Context, Result, bail};
use corral_core::{CheckpointRecord, Run, RunProgress, Task};
use serde::Serialize;
use serde_json::json;

use crate::{
    Ctx,
    progress::{progress_spec_revision, tree_revision},
};

pub use corral_core::ResumeStatus;

#[derive(Debug, Clone, Serialize)]
pub struct ResumePacket {
    pub version: u32,
    pub task: String,
    pub run: String,
    pub spec_revision: String,
    /// Always present even when a harness overrides the human prompt template.
    pub canonical_title: String,
    pub canonical_spec: String,
    pub status: ResumeStatus,
    pub reasons: Vec<String>,
    /// Historical data stays available even when incompatible, including cleanup obligations.
    pub checkpoint: Option<CheckpointRecord>,
    /// When the newest checkpoint is stale, this older compatible snapshot supplies
    /// progress only. The newest checkpoint above still owns handoff obligations.
    pub compatible_checkpoint: Option<CheckpointRecord>,
    /// Only a compatible checkpoint supplies authoritative execution progress.
    pub progress: Option<RunProgress>,
    /// References are not PASS claims; the acceptance ledger remains authoritative.
    pub reusable_evidence: Vec<String>,
}

pub(crate) fn stale_reasons(
    task: &Task,
    run: &Run,
    record: &CheckpointRecord,
) -> Result<Vec<String>> {
    let checkpoint = &record.checkpoint;
    let input = &checkpoint.input;
    let mut reasons = Vec::new();
    if input.task != task.id {
        reasons.push("task_changed".into());
    }
    if input.spec_revision != progress_spec_revision(task)? {
        reasons.push("spec_changed".into());
    }
    if input.head != corral_git::rev_sha(&run.worktree, "HEAD")? {
        reasons.push("head_changed".into());
    }
    if input.base != corral_git::rev_sha(&run.worktree, &run.base)? {
        reasons.push("base_changed".into());
    }
    if input.tree != tree_revision(run)? {
        reasons.push("tree_changed".into());
    }
    Ok(reasons)
}

/// Build a packet for the actual target worktree, not the previous worker's tree.
pub fn resume_packet(_ctx: &Ctx, task: &Task, run: &Run) -> Result<ResumePacket> {
    let source = task.current_run();
    let checkpoint = source.and_then(|r| r.checkpoints.last()).cloned();
    let mut packet = ResumePacket {
        version: 1,
        task: task.id.clone(),
        run: run.id.clone(),
        spec_revision: progress_spec_revision(task)?,
        canonical_title: task.title.clone(),
        canonical_spec: task.body.clone(),
        status: ResumeStatus::Missing,
        reasons: vec!["checkpoint_missing".into()],
        checkpoint: checkpoint.clone(),
        compatible_checkpoint: None,
        progress: None,
        reusable_evidence: Vec::new(),
    };
    let assess = |record: &CheckpointRecord| -> Result<Vec<String>> {
        let mut reasons = stale_reasons(task, run, record)?;
        if source.is_some_and(|s| s.id != run.id && !s.worktree.is_dir()) {
            reasons.push("source_worktree_missing".into());
        }
        if !task.runs.iter().any(|r| {
            r.id == record.checkpoint.input.run
                && r.checkpoints.iter().any(|c| c.artifact == record.artifact)
        }) {
            reasons.push("source_run_changed".into());
        }
        if source
            .and_then(|s| s.progress.as_ref())
            .map_or(0, |p| p.revision)
            != record.checkpoint.input.progress_revision
        {
            reasons.push("progress_changed".into());
        }
        Ok(reasons)
    };
    if let Some(record) = &checkpoint {
        packet.reasons = assess(record)?;
        packet.status = ResumeStatus::Stale;
    }
    if let Some(source) = source {
        for record in source.checkpoints.iter().rev() {
            if !assess(record)?.is_empty() {
                continue;
            }
            packet.status = ResumeStatus::Compatible;
            packet.progress = record.checkpoint.progress.clone();
            if checkpoint
                .as_ref()
                .is_some_and(|latest| latest.artifact != record.artifact)
            {
                packet.compatible_checkpoint = Some(record.clone());
                packet
                    .reasons
                    .push("using_older_compatible_checkpoint".into());
            }
            // Evidence is never moved to the new run or promoted to a completion receipt.
            if source.id == run.id && record.checkpoint.input.run == run.id {
                packet.reusable_evidence = record.checkpoint.input.evidence_refs.clone();
            }
            break;
        }
    }
    Ok(packet)
}

/// A worker calling `done` is reachable by definition. Retry uses this only for a live pane.
pub(crate) fn require_checkpoint(task: &Task, run: &Run) -> Result<()> {
    if !run.checkpoint_required {
        return Ok(());
    }
    let Some(record) = run.checkpoints.last() else {
        bail!(
            "checkpoint_required: run {}; use corral run checkpoint {} --file <json>",
            run.id,
            task.id
        );
    };
    let mut reasons = stale_reasons(task, run, record)?;
    if record.checkpoint.input.run != run.id {
        reasons.push("source_run_changed".into());
    }
    if run.progress.as_ref().map_or(0, |p| p.revision) != record.checkpoint.input.progress_revision
    {
        reasons.push("progress_changed".into());
    }
    if !reasons.is_empty() {
        bail!(
            "checkpoint_stale: {}; record a new checkpoint",
            reasons.join(",")
        );
    }
    Ok(())
}

pub(crate) fn retry_checkpoint(ctx: &Ctx, task: &Task, run: &Run) -> Result<()> {
    if let Err(error) = require_checkpoint(task, run) {
        let _delivery_lock = ctx
            .store
            .try_lock(&format!("delivery-{}", task.id))?
            .context("another reply is being delivered; try again")?;
        let pane = &run.herdr.as_ref().context("missing_worker_pane")?.pane_id;
        let request = format!(
            "[corral] Before retrying {} run {}, record a portable checkpoint with \
             `corral run checkpoint {} --file <json>`. Preserve pending waits, owned resources, \
             decisions, and a concrete next action. Retry has not superseded this run.",
            task.id, run.id, task.id
        );
        ctx.store.update(&task.id, |t| {
            let current = t
                .current_run_mut()
                .filter(|r| r.id == run.id)
                .context("stale_run before checkpoint request")?;
            if current
                .pending_reply
                .as_ref()
                .is_some_and(|pending| pending != &request)
            {
                bail!("pending_reply_before_checkpoint_request: deliver the existing reply first");
            }
            current.pending_reply = Some(request.clone());
            Ok(())
        })?;
        let delivery = ctx.herdr.agent_prompt(pane, &request)?;
        ctx.store.log_event(
            Some(&task.id),
            "checkpoint_requested",
            json!({
                "run": run.id, "pane": pane, "via": delivery.via,
                "submitted": delivery.submitted, "attempts": delivery.attempts,
                "error": delivery.error,
            }),
        )?;
        if !delivery.submitted {
            bail!(
                "checkpoint_request_not_delivered: {}; pending request retained for retry",
                delivery
                    .error
                    .as_deref()
                    .unwrap_or("submission was not observed")
            );
        }
        ctx.store.update(&task.id, |t| {
            let current = t
                .current_run_mut()
                .filter(|r| r.id == run.id)
                .context("stale_run after checkpoint request")?;
            if current.pending_reply.as_ref() == Some(&request) {
                current.pending_reply = None;
            }
            Ok(())
        })?;
        return Err(error);
    }
    Ok(())
}
