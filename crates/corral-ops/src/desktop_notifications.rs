//! Desktop attention is independent of herdr notifications and PM event delivery.
//!
//! Store updates do not have a uniform transition event, so checkpoint observed task
//! statuses instead of interpreting individual producers' event kinds. Sample before
//! sync advances Review cards as well as after it reconciles agents.

use anyhow::{Context, Result};
use corral_core::Status;
use corral_core::records::RecordArea;
use std::collections::BTreeMap;

use crate::Ctx;

const OBSERVATIONS: RecordArea<BTreeMap<String, Status>> = RecordArea::new("desktop-notifications");

pub(crate) fn reconcile(ctx: &Ctx) -> Result<()> {
    let Some(_lock) = ctx.store.try_lock("desktop-notifications")? else {
        return Ok(());
    };
    let previous = OBSERVATIONS.load(&ctx.store, "state")?;
    let tasks = ctx.store.list_shared()?;
    let current: BTreeMap<_, _> = tasks.iter().map(|t| (t.id.clone(), t.status)).collect();
    if previous.as_ref() == Some(&current) {
        return Ok(());
    }
    // Claim the observations before invoking an external process. Delivery is
    // best-effort, at most once per observed entry, including transport failure.
    // Disabled entries are checkpointed too, never queued for later enablement.
    OBSERVATIONS.save(&ctx.store, "state", &current)?;
    let previous = previous.unwrap_or_default();

    for task in tasks.iter() {
        if previous.get(&task.id) == Some(&task.status) {
            continue;
        }
        let enabled = match task.status {
            Status::NeedsMe => ctx.store.config.ui.notify_needs_me,
            Status::Review => ctx.store.config.ui.notify_review,
            Status::Idea
            | Status::Backlog
            | Status::Ready
            | Status::Running
            | Status::Done
            | Status::Dropped => false,
        };
        if enabled
            && let Err(err) = notify(
                &format!("{}: {}", task.id, task.status.label()),
                &task.title,
            )
        {
            crate::messages::report(
                Some(&task.id),
                "desktop_notification",
                format!("desktop notification for {} failed: {err:#}", task.id),
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn notify(summary: &str, body: &str) -> Result<()> {
    use std::process::{Command, Stdio};

    let status = Command::new("notify-send")
        .args(["--app-name=corral", "--", summary, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running notify-send")?;
    anyhow::ensure!(status.success(), "notify-send exited with {status}");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn notify(_summary: &str, _body: &str) -> Result<()> {
    anyhow::bail!("desktop notifications require Linux notify-send")
}
