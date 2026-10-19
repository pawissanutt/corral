//! Formatting helpers for autopilot diagnostics.

use anyhow::{Context, Result};
use corral_core::{ArtifactKind, Task};
use serde_json::json;

use super::Pilot;
use crate::Ctx;

pub(super) const LOG_TAIL_LINES: usize = 30;

/// Report authority transitions once per run, including concurrent sync callers.
pub(super) fn record_authority(ctx: &Ctx, task: &Task) -> Result<()> {
    let Some(_lock) = ctx
        .store
        .try_lock(&format!("autopilot-status-{}", task.id))?
    else {
        return Ok(());
    };
    let run = task.current_run().map(|run| run.id.as_str());
    let previous = ctx.store.events()?.into_iter().rev().find(|event| {
        event.task.as_deref() == Some(task.id.as_str())
            && event.detail["run"].as_str() == run
            && matches!(
                event.kind.as_str(),
                "automation_blocked" | "automation_resumed"
            )
    });
    if let Some(reason) = crate::automation_blocker(ctx, task)? {
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
    } else if previous.is_some_and(|event| event.kind == "automation_blocked") {
        ctx.store
            .log_event(Some(&task.id), "automation_resumed", json!({"run": run}))?;
    }
    Ok(())
}

/// Persist once across sync callers, not once per worker or process.
pub(super) fn record_error(ctx: &Ctx, task: &Task, error: &str) -> Result<()> {
    let Some(_lock) = ctx
        .store
        .try_lock(&format!("autopilot-error-{}", task.id))?
    else {
        return Ok(());
    };
    let run = task.current_run().map(|r| r.id.as_str());
    let previous = ctx.store.events()?.into_iter().rev().find(|event| {
        event.task.as_deref() == Some(task.id.as_str())
            && event.kind == "autopilot_error"
            && event.detail["run"].as_str() == run
    });
    if previous.is_none_or(|event| event.detail["error"].as_str() != Some(error)) {
        ctx.store.log_event(
            Some(&task.id),
            "autopilot_error",
            json!({"run": run, "error": error}),
        )?;
    }
    Ok(())
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
        let tmp = self.ctx.store.state_dir().join("tmp").join(format!(
            "{}-{}-autopilot",
            self.id(),
            self.run.id
        ));
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
