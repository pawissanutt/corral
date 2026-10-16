//! Desktop attention is independent of herdr notifications and PM event delivery.
//!
//! Store updates do not have a uniform transition event, so checkpoint observed task
//! statuses instead of interpreting individual producers' event kinds. Sample before
//! sync advances Review cards as well as after it reconciles agents.

use std::collections::BTreeMap;
use std::io::ErrorKind;

use anyhow::{Context, Result};
use corral_core::Status;

use crate::Ctx;

pub(crate) fn reconcile(ctx: &Ctx) -> Result<()> {
    let Some(_lock) = ctx.store.try_lock("desktop-notifications")? else {
        return Ok(());
    };
    let path = ctx.store.state_dir().join("desktop-notifications.json");
    let previous: BTreeMap<String, Status> = match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?
        }
        Err(err) if err.kind() == ErrorKind::NotFound => BTreeMap::new(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let tasks = ctx.store.list()?;
    let current: BTreeMap<_, _> = tasks.iter().map(|t| (t.id.clone(), t.status)).collect();
    if current == previous && path.exists() {
        return Ok(());
    }
    // Claim the observations before invoking an external process. Delivery is
    // best-effort, at most once per observed entry, including transport failure.
    // Disabled entries are checkpointed too, never queued for later enablement.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec(&current).context("serializing desktop notification state")?,
    )
    .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("saving {}", path.display()))?;

    for task in tasks {
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
