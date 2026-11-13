//! Formatting helpers for autopilot diagnostics.

use anyhow::{Context, Result};
use corral_core::{ArtifactKind, TaskHead};
use serde_json::json;

use super::Pilot;
use crate::Ctx;

const LOG_TAIL_LINES: usize = 30;
/// The markers `scripts/gate_repair.py` prints around a failed test run's rerun commands.
const REPAIR_START: &str = "Repair commands (diagnostic only, not a gate result):";
const REPAIR_END: &str = "End of repair commands.";
const REPAIR_GUIDANCE: &str = "Rerun the gate's repair commands above after each edit until \
    they pass: each builds only the failing test's binary and runs only its failed tests, and is \
    not a gate result. Then continue as instructed below; do not start a gate to check each edit.";

/// Report authority transitions once per run, including concurrent sync callers.
/// The open Review tasks, as heads: autopilot never reads a terminal task. The open board is
/// refreshed incrementally: only heads changed since the previous read are decoded.
pub(super) fn review_tasks(ctx: &Ctx) -> Result<Vec<TaskHead>> {
    Ok(ctx
        .store
        .cached_board()?
        .heads
        .into_iter()
        .filter(|head| head.status == corral_core::Status::Review && head.hold.is_none())
        .map(TaskHead::detach)
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
    let reason = ctx.read_step(|read| {
        crate::automation_status::automation_blocker_ref(read, &read.store.head(&task.id)?)
    })?;
    if let Some(reason) = reason {
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

/// A failed gate's send-back details: its log's tail, plus the repair commands the gate printed
/// when they fall outside that tail, and the repair-first guidance whenever there are any.
pub(super) fn failed_gate_details(
    ctx: &Ctx,
    task: &str,
    run: &str,
    gate: &corral_core::GateRecord,
) -> Result<String> {
    if let Some(identity) = &gate.identity
        && let Some(bytes) = crate::gate::evidence::attempt_log(ctx, task, run, &identity.attempt)?
    {
        return Ok(log_details(
            &format!("retained attempt {}", identity.attempt),
            &String::from_utf8_lossy(&bytes),
        ));
    }
    Ok(gate_log_details(
        &crate::gate::gate_paths(ctx, task, run).log,
    ))
}

pub(super) fn gate_log_details(path: &std::path::Path) -> String {
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => format!("(could not read {}: {e})", path.display()),
    };
    log_details(&path.display().to_string(), &text)
}

fn log_details(name: &str, text: &str) -> String {
    let all: Vec<&str> = text.lines().collect();
    let tail = all[all.len().saturating_sub(LOG_TAIL_LINES)..].join("\n");
    let mut details =
        format!("Last {LOG_TAIL_LINES} lines of the gate log ({name}):\n\n```\n{tail}\n```");
    if let Some(block) = repair_block(&all) {
        if !tail.contains(&block) {
            details.push_str(&format!(
                "\n\nRepair commands from the gate log:\n\n```\n{block}\n```"
            ));
        }
        details.push_str(&format!("\n\n{REPAIR_GUIDANCE}"));
    }
    details
}

/// The log's last complete repair-commands block, markers included.
fn repair_block(lines: &[&str]) -> Option<String> {
    let start = lines.iter().rposition(|l| l.trim_end() == REPAIR_START)?;
    let end = start
        + lines[start..]
            .iter()
            .position(|l| l.trim_end() == REPAIR_END)?;
    Some(lines[start..=end].join("\n"))
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

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: [&str; 3] = [REPAIR_START, "(rerun alpha)", REPAIR_END];

    #[test]
    fn repair_commands_outside_the_tail_are_carried_with_guidance() {
        let mut lines = vec!["test alpha ... FAILED"];
        lines.extend(BLOCK);
        lines.extend(std::iter::repeat_n("timer line", LOG_TAIL_LINES));
        let details = log_details("gate.log", &lines.join("\n"));
        assert!(details.contains("Repair commands from the gate log:"));
        assert!(details.contains("(rerun alpha)"));
        assert!(details.ends_with(REPAIR_GUIDANCE));
    }

    #[test]
    fn repair_commands_in_the_tail_are_not_repeated() {
        let mut lines = vec!["timer line"];
        lines.extend(BLOCK);
        lines.push("FAILED: nextest (exit 100)");
        let details = log_details("gate.log", &lines.join("\n"));
        assert_eq!(details.matches("(rerun alpha)").count(), 1);
        assert!(!details.contains("Repair commands from the gate log:"));
        assert!(details.ends_with(REPAIR_GUIDANCE));
    }

    #[test]
    fn no_complete_repair_block_means_no_guidance() {
        for text in [
            "FAILED: clippy (exit 101)",
            &format!("{REPAIR_START}\n(rerun alpha)"),
        ] {
            let details = log_details("gate.log", text);
            assert!(!details.contains(REPAIR_GUIDANCE), "{details}");
        }
    }
}
