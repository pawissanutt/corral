//! Launch receipts. A launched agent command that returns to its pane shell before reporting (a
//! failed exec such as an over-long argument, or a crash) leaves a receipt, so sync can end the
//! attempt instead of leaving it open with no process behind it.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{ArtifactKind, PlanRunState, Status, Task, Verdict};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// The receipt the pane shell writes when the command of `attempt` returns.
pub(crate) fn receipt(ctx: &Ctx, task: &str, attempt: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("launch-exits")
        .join(format!("{task}-{attempt}.exited"))
}

/// `command` followed by a receipt written only when it fails: a refused exec (126/127, E2BIG) or a
/// crash. `||` and `>` mean the same in sh, bash, zsh and fish, unlike `$?`. A `cat` display returns
/// to the shell by design (the generic harness), so it is not armed.
pub(crate) fn arm(command: &str, receipt: &Path) -> Result<String> {
    if crate::harness::program_of(command).as_deref() == Some("cat") {
        return Ok(command.to_string());
    }
    let dir = receipt
        .parent()
        .context("launch receipt has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    match std::fs::remove_file(receipt) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("clearing {}", receipt.display()));
        }
    }
    let path = crate::prompt::shell_path(receipt);
    Ok(format!(
        "{command} || true > '{}'",
        crate::prompt::sh_escape(&path)
    ))
}

pub(crate) fn exited(receipt: &Path) -> bool {
    receipt.is_file()
}

/// Why the attempt ended: the shell's last lines carry the exec error.
pub(crate) fn reason(ctx: &Ctx, what: &str, pane: Option<&str>) -> String {
    let capture = match pane.map(|p| ctx.herdr.pane_read_if_present(p, 80)) {
        Some(Ok(Some(text))) => {
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            lines[lines.len().saturating_sub(12)..].join("\n")
        }
        Some(Ok(None)) => "(the pane is no longer present)".into(),
        Some(Err(error)) => format!("(could not read the pane: {error:#})"),
        None => "(no pane was recorded)".into(),
    };
    format!("{what} exited to its shell before reporting.\n{capture}")
}

/// Keep the reason as a run artifact so it survives the pane.
pub(crate) fn record(ctx: &Ctx, task: &str, run: &str, title: &str, reason: &str) -> Result<()> {
    let dir = ctx.store.state_dir().join("launch-exits");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!(
        "{task}-{run}-{}.log",
        Timestamp::now().as_millisecond()
    ));
    std::fs::write(&path, format!("{task} {run}: {reason}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    crate::artifacts::add_artifact_for_run(
        ctx,
        task,
        run,
        &path,
        Some(ArtifactKind::Log),
        Some(title),
    )?;
    Ok(())
}

/// End a pending plan-review attempt whose reviewer returned to its shell, exactly as a failed
/// launch does: an `escalate` verdict carrying the error, a failed plan run, and Needs me.
pub(crate) fn plan_review(ctx: &Ctx, task: &Task) -> Result<Option<crate::Change>> {
    let Some(plan) = task.plans.last() else {
        return Ok(None);
    };
    let Some(review) = plan
        .reviews
        .last()
        .filter(|r| r.verdict.is_none() && r.reviewer != "pm")
    else {
        return Ok(None);
    };
    let attempt = format!("{}-{}", plan.id, review.id);
    if !exited(&receipt(ctx, &task.id, &attempt)) {
        return Ok(None);
    }
    let pane = review.herdr.as_ref().map(|h| h.pane_id.as_str());
    let reason = reason(ctx, "The plan reviewer", pane);
    let saved = ctx.store.try_update(&task.id, |t| {
        if !crate::session_scope::owns_task(ctx, t)? {
            return Ok(false);
        }
        let Some(p) = t.plans.last_mut().filter(|p| p.id == plan.id) else {
            return Ok(false);
        };
        let Some(r) = p
            .reviews
            .last_mut()
            .filter(|r| r.id == review.id && r.verdict.is_none())
        else {
            return Ok(false);
        };
        r.verdict = Some(Verdict::Escalate);
        r.notes = Some(format!("Review launch failed: {reason}"));
        r.pending_reply = None;
        r.at = Timestamp::now();
        let run_id = p.run.clone();
        if let Some(run) = t
            .plan_runs
            .iter_mut()
            .find(|r| Some(&r.id) == run_id.as_ref() && r.state != PlanRunState::Cancelled)
        {
            run.state = PlanRunState::Failed;
        }
        if t.status == Status::Review {
            t.status = Status::NeedsMe;
        }
        Ok(true)
    })?;
    let Some(saved) = saved else {
        return Ok(None);
    };
    if let Some(p) = saved.plans.last() {
        crate::plan_review::close_plan_review(ctx, p)?;
    }
    ctx.store.log_event(
        Some(&task.id),
        "plan_review_failed",
        json!({"target":"plan","plan":plan.id,"attempt":review.id,"error":reason}),
    )?;
    if saved.status == task.status {
        return Ok(None);
    }
    ctx.store.log_event(
        Some(&task.id),
        "needs_me",
        json!({"target":"plan","plan":plan.id,"from":task.status,"note":reason}),
    )?;
    Ok(Some(crate::Change {
        task: task.id.clone(),
        from: task.status,
        to: saved.status,
        note: reason,
        stage: None,
    }))
}
