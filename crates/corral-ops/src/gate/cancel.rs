//! Intentional cancellation is a terminal attempt outcome, never a gate verdict.
use anyhow::Result;
use corral_core::GateResult;
use serde_json::json;

use super::{attempt_paths, attempt_stem, evidence};
use crate::Ctx;

#[cfg(test)]
#[path = "cancel_tests.rs"]
mod tests;

pub(crate) fn cancel(ctx: &Ctx, id: &str, reason: &str) -> Result<bool> {
    let task = ctx.store.head(id)?;
    let Some(run) = task.current_run.as_deref() else {
        return Ok(false);
    };
    let Some(gate) = run
        .gate
        .as_ref()
        .filter(|g| g.result == GateResult::Running)
    else {
        return Ok(false);
    };
    // Never close the worker even for a malformed legacy record.
    if let Some(pane) = gate.pane.as_deref()
        && run.herdr.as_ref().is_none_or(|h| h.pane_id != pane)
        && ctx.herdr.pane_get(pane)?.is_some()
    {
        ctx.herdr.pane_close(pane)?;
    }
    let staged = if let Some(identity) = &gate.identity {
        let stem = attempt_stem(id, &run.id, Some(&identity.attempt));
        let paths = attempt_paths(ctx, id, &run.id, Some(&identity.attempt));
        stop_descendants(&paths.script)?;
        evidence::put_result(ctx, id, &run.id, &identity.attempt, &stem, &paths)?
    } else {
        None
    };
    let updated = ctx.store.try_update_head(id, |task| {
        let Some(current) = task.current_run_mut().filter(|r| r.id == run.id) else { return Ok(false); };
        if current.gate.as_ref() != Some(gate) { return Ok(false); }
        let gate = current.gate.as_mut().ok_or_else(|| anyhow::anyhow!("gate disappeared"))?;
        gate.result = GateResult::Cancelled;
        gate.process_lost = false;
        gate.failed_cmd = None;
        gate.at = ctx.store.now();
        crate::gate_triage::observe(&ctx.store, id, gate)?;
        if let Some(staged) = &staged { staged.stage(&ctx.store, (GateResult::Cancelled, None))?; }
        ctx.store.log_event(Some(id), "gate_cancelled", json!({"run":run.id,"attempt":gate.identity.as_ref().map(|i| &i.attempt),"result":"cancelled","reason":reason}))?;
        crate::agent::gate_queue::requeue_cancelled(ctx, task)?;
        Ok(true)
    })?;
    Ok(updated.is_some())
}

/// Gate descendants have their own immutable scratch-script marker (unique across stores,
/// including aggregate/checkpoint attempts whose local names are just A1/C1). The worker shares the run's
/// process-owner marker, so it must never be selected by that broader marker here.
#[cfg(target_os = "linux")]
pub(crate) fn stop_descendants(script: &std::path::Path) -> Result<()> {
    use crate::watchdog::{ProcessControl, SystemProcesses};
    use anyhow::Context;
    let protected = crate::cleanup_worktree::protected_pids()?;
    let token = format!(
        "CORRAL_GATE_PROCESS_OWNER={}",
        crate::prompt::shell_path(script)
    );
    for entry in std::fs::read_dir("/proc").context("listing gate descendants")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if protected.contains(&pid) {
            continue;
        }
        let matches = || {
            std::fs::read(entry.path().join("environ"))
                .ok()
                .is_some_and(|env| env.split(|b| *b == 0).any(|v| v == token.as_bytes()))
        };
        if !matches() {
            continue;
        }
        let Some(snapshot) = SystemProcesses.inspect(pid)? else {
            continue;
        };
        let process = corral_core::StuckProcess {
            pid,
            argv: snapshot.argv,
            started: snapshot.started,
        };
        if matches() {
            SystemProcesses
                .terminate(&process)
                .context("stopping gate descendant")?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn stop_descendants(_script: &std::path::Path) -> Result<()> {
    Ok(())
}
