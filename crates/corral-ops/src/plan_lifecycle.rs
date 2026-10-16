//! Planner process receipts, launch observation and terminal outcomes.
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use corral_core::{ArtifactKind, PlanRun, PlanRunState, Status, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::{Ctx, prompt};

fn exit_path(run: &PlanRun) -> PathBuf {
    run.output_dir.join(format!("exit-{}.status", run.rounds))
}

pub(crate) fn record_exit(run: &PlanRun, command: &str) -> String {
    format!(
        "{command}; corral_plan_exit=$?; printf '%s\\n' \"$corral_plan_exit\" > '{}'",
        prompt::sh_escape(&exit_path(run).display().to_string())
    )
}

pub(crate) fn exit_reason(run: &PlanRun) -> Result<Option<String>> {
    let path = exit_path(run);
    let status = match std::fs::read_to_string(&path) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading planner exit receipt"),
    };
    if status.trim().is_empty() {
        return Ok(None);
    }
    let code: u32 = status
        .trim()
        .parse()
        .context("parsing planner exit status")?;
    Ok(Some(format!(
        "Planner exited with status {code} before plan submission."
    )))
}

pub(crate) fn shell_returned(ctx: &Ctx, run: &PlanRun) -> Result<bool> {
    let Some(herdr) = &run.herdr else {
        return Ok(false);
    };
    let info = ctx.herdr.pane_process_info(&herdr.pane_id)?;
    Ok(!info
        .foreground_processes
        .iter()
        .any(|process| process.pid != info.shell_pid))
}

/// Register before submission so even an immediate crash cannot beat observation.
pub(crate) struct Smoke {
    fd: rustix::fd::OwnedFd,
}

impl Smoke {
    pub(crate) fn new(run: &PlanRun) -> Result<Self> {
        use rustix::fs::inotify;
        let path = exit_path(run);
        std::fs::write(&path, "").context("initializing planner exit receipt")?;
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC)
            .context("creating planner launch observation")?;
        inotify::add_watch(&fd, &path, inotify::WatchFlags::CLOSE_WRITE)
            .context("watching planner exit receipt")?;
        Ok(Self { fd })
    }

    pub(crate) fn check(self, ctx: &Ctx, task: &Task) -> Result<()> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let mut fds = [PollFd::new(&self.fd, PollFlags::IN)];
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 500_000_000,
        };
        poll(&mut fds, Some(&timeout)).context("observing planner launch window")?;
        // Submission can finish inside the window and close its own pane.
        let current = ctx.store.get(&task.id)?;
        let run = task.plan_runs.last().context("planner run missing")?;
        if current
            .plan_runs
            .last()
            .is_none_or(|r| r.id != run.id || r.started != run.started || !r.holds_slot())
        {
            return Ok(());
        }
        if let Some(reason) = exit_reason(run)? {
            bail!("{reason}");
        }
        ensure!(
            ctx.herdr
                .pane_get(&run.herdr.as_ref().context("planner pane missing")?.pane_id)?
                .is_some(),
            "Planner pane disappeared during launch."
        );
        ensure!(
            !shell_returned(ctx, run)?,
            "Planner returned to a shell before plan submission."
        );
        Ok(())
    }
}

pub(crate) fn fail(ctx: &Ctx, task: &Task, reason: &str, now: Timestamp) -> Result<bool> {
    let run = task.plan_runs.last().context("planner run missing")?;
    let mut failed = false;
    let saved = ctx.store.update(&task.id, |t| {
        let Some(current) = t
            .plan_runs
            .last_mut()
            .filter(|r| r.id == run.id && r.started == run.started && r.holds_slot())
        else {
            return Ok(());
        };
        current.state = PlanRunState::Failed;
        current.ended = Some(now);
        current.pending_reply = Some(reason.into());
        t.status = Status::NeedsMe;
        t.queued = false;
        failed = true;
        Ok(())
    })?;
    if !failed {
        return Ok(false);
    }
    let run = saved
        .plan_runs
        .last()
        .context("failed planner run missing")?;
    std::fs::create_dir_all(&run.output_dir).context("creating planner failure directory")?;
    let capture = match &run.herdr {
        Some(herdr) => match ctx.herdr.pane_read_if_present(&herdr.pane_id, 100) {
            Ok(Some(text)) => text,
            Ok(None) => "Planner pane is no longer present.".into(),
            Err(error) => format!("Could not capture planner pane: {error:#}"),
        },
        None => "Planner launch did not attach a pane.".into(),
    };
    let path = run.output_dir.join(format!("failure-{}.log", run.rounds));
    std::fs::write(
        &path,
        format!("{} {}: {reason}\n\n{capture}\n", task.id, run.id),
    )
    .context("writing planner failure evidence")?;
    let artifact = crate::artifacts::add_artifact_for_run(
        ctx,
        &task.id,
        &run.id,
        &path,
        Some(ArtifactKind::Log),
        Some("Planner failure"),
    )?;
    ctx.store.log_event(
        Some(&task.id),
        "plan_failed",
        json!({"target":"plan","run":run.id,"reason":reason,"artifact":artifact.path}),
    )?;
    ctx.store.log_event(
        Some(&task.id),
        "needs_me",
        json!({"target":"plan","run":run.id,"from":task.status,"note":reason}),
    )?;
    Ok(true)
}

/// Cancel only the planner; submitted plans and independent verdicts remain intact.
pub fn plan_cancel(ctx: &Ctx, id: &str) -> Result<Task> {
    let _ownership = ctx
        .store
        .try_lock("session-ownership")?
        .context("pane inventory is being reconciled; retry plan cancellation")?;
    let _delivery = ctx
        .store
        .try_lock(&format!("delivery-{id}"))?
        .context("planner input is being delivered; retry plan cancellation")?;
    let task = ctx.store.get(id)?;
    ensure!(
        crate::session_scope::owns_task(ctx, &task)?,
        "selected session does not own plan pane"
    );
    let run = task
        .plan_runs
        .last()
        .context("task has no plan run to cancel")?;
    ensure!(
        run.state != PlanRunState::Approved,
        "approved plan run cannot be cancelled"
    );
    if run.state == PlanRunState::Cancelled {
        return Ok(task);
    }
    crate::plan_dispatch::close_planner(ctx, &task)?;
    let saved = ctx.store.update(id, |t| {
        let current = t
            .plan_runs
            .last_mut()
            .filter(|r| r.id == run.id && r.started == run.started)
            .context("planner changed during cancellation")?;
        current.state = PlanRunState::Cancelled;
        current.ended = Some(Timestamp::now());
        current.pending_reply = Some("Plan run cancelled.".into());
        t.queued = false;
        t.dispatch_plan = None;
        t.dispatch_hold = true;
        t.status = if t.plans.last().is_some_and(|p| p.approval.is_none()) {
            Status::NeedsMe
        } else {
            Status::Ready
        };
        Ok(())
    })?;
    ctx.store.log_event(
        Some(id),
        "plan_cancelled",
        json!({"target":"plan","run":run.id}),
    )?;
    Ok(saved)
}
