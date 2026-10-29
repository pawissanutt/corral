//! Formatting helpers for autopilot diagnostics.

use anyhow::{Context, Result};
use corral_core::{ArtifactKind, TaskHead};
use serde_json::json;

use super::Pilot;
use crate::Ctx;

pub(super) const LOG_TAIL_LINES: usize = 30;

/// Report authority transitions once per run, including concurrent sync callers.
/// The open Review tasks, as heads: autopilot never reads a terminal task.
pub(super) fn review_tasks(ctx: &Ctx) -> Result<Vec<TaskHead>> {
    Ok(ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
        .into_iter()
        .filter(|head| head.status == corral_core::Status::Review)
        .collect())
}

/// Whether the integration request `digest` was already sent for `key`'s run.
pub(super) fn requested(
    ctx: &Ctx,
    key: &crate::integration::RepairKey,
    digest: &str,
) -> Result<bool> {
    let matches = |event: &corral_core::Event| event.detail["digest"].as_str() == Some(digest);
    Ok(ctx
        .store
        .last_event_where(
            Some(&key.task),
            &["integration_request"],
            Some(&key.run),
            matches,
        )?
        .is_some())
}

pub(super) fn record_authority(ctx: &Ctx, task: &TaskHead) -> Result<()> {
    let Some(_lock) = ctx
        .store
        .try_lock(&format!("autopilot-status-{}", task.id))?
    else {
        return Ok(());
    };
    let run = task.current_run.as_ref().map(|run| run.id.as_str());
    // The run's newest automation event, without reading the task's earlier runs' events.
    let previous = ctx.store.last_event_where(
        Some(&task.id),
        &["automation_blocked", "automation_resumed"],
        run,
        |event| event.detail["run"].as_str() == run,
    )?;
    let previous = previous.as_ref();
    if let Some(reason) = crate::automation_status::automation_blocker_ref(ctx, task)? {
        if previous.is_none_or(|event| {
            event.kind != "automation_blocked"
                || event.detail["reason"].as_str() != Some(reason.as_str())
        }) {
            ctx.store.log_event(
                Some(&task.id),
                "automation_blocked",
                json!({"run": run, "reason": reason}),
            )?;
        }
    } else if previous.is_some_and(|event| {
        // An instruction conflict is resumed by the clean scan that resolves it, not here.
        event.kind == "automation_blocked" && !crate::gate_tier::is_conflict_block(event)
    }) {
        ctx.store
            .log_event(Some(&task.id), "automation_resumed", json!({"run": run}))?;
    }
    Ok(())
}

/// A repeat of the open run's error counts on its condition run without a new event.
pub(super) fn record_error(ctx: &Ctx, task: &TaskHead, error: &str) -> Result<()> {
    let run = task.current_run.as_ref().map(|r| r.id.as_str());
    ctx.store.log_event(
        Some(&task.id),
        "autopilot_error",
        json!({"run": run, "error": error}),
    )
}

/// A clean autopilot pass over a task resolves its open `autopilot_error` run.
pub(super) fn record_clean(ctx: &Ctx, task: &TaskHead) -> Result<()> {
    ctx.store
        .clear_condition(Some(&task.id), "autopilot_error")
        .map(drop)
}

pub(super) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

pub(super) fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

pub(super) fn log_tail(path: &std::path::Path, lines: usize) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(e) => format!("(could not read {}: {e})", path.display()),
    }
}

impl Pilot<'_> {
    pub(super) fn snapshot(
        &self,
        name: &str,
        text: &str,
        title: &str,
    ) -> Result<corral_core::Artifact> {
        let tmp =
            self.ctx
                .store
                .scratch_tmp()?
                .join(format!("{}-{}-autopilot", self.id(), self.run.id));
        std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let file = tmp.join(name);
        let added = std::fs::write(&file, text)
            .with_context(|| format!("writing {}", file.display()))
            .and_then(|()| {
                crate::artifacts::add_artifact_for_run(
                    self.ctx,
                    self.id(),
                    &self.run.id,
                    &file,
                    Some(ArtifactKind::Report),
                    Some(title),
                )
            });
        if let Err(e) = std::fs::remove_dir_all(&tmp) {
            crate::messages::report(
                Some(self.id()),
                "cleanup",
                format!("could not remove {} (ignored): {e}", tmp.display()),
            );
        }
        added
    }
}
