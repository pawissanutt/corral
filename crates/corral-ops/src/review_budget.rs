//! Review admission is independent of coding slots and dispatch pause.
//! The lock covers admission through launch; the journal covers launcher crashes.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use corral_core::{GateResult, Run};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;

pub(crate) use corral_core::ReviewWork as Kind;

#[derive(Debug)]
pub(crate) struct CapacityWait {
    pub kind: Kind,
    pub reason: String,
}

impl std::fmt::Display for CapacityWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "waiting for a slot: {}", self.reason)
    }
}
impl std::error::Error for CapacityWait {}

#[derive(Serialize, Deserialize)]
struct Pending {
    task: String,
    run: String,
    kind: Kind,
    pane: Option<String>,
    // A PM pane belongs to the workspace, not this review.
    close_pane: bool,
    #[serde(default)]
    started: bool,
}

pub(crate) struct ReviewLaunch<'a> {
    ctx: &'a Ctx,
    _lock: corral_core::StoreLock,
    pending: Pending,
    armed: bool,
    committed: bool,
}

fn journal(ctx: &Ctx) -> PathBuf {
    ctx.store.state_dir().join("review-launch.json")
}

fn save(ctx: &Ctx, pending: &Pending) -> Result<()> {
    let path = journal(ctx);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(pending)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("saving {}", path.display()))
}

fn clear(ctx: &Ctx) -> Result<()> {
    let path = journal(ctx);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

fn finish(ctx: &Ctx, run: &mut Run, kind: Kind) {
    match kind {
        Kind::Gate => {
            if let Some(gate) = &mut run.gate
                && gate.result == GateResult::Running
            {
                gate.process_lost = true;
            }
        }
        Kind::Reviewer => {
            if run.review.as_ref().is_some_and(|r| r.verdict.is_none()) {
                run.review = None;
                if let Some(snapshot) = run.review_runs.last_mut()
                    && snapshot.ended.is_none()
                {
                    snapshot.ended = Some(Timestamp::now());
                    crate::usage::collect(snapshot, &ctx.store);
                }
            }
        }
    }
}

fn abandon(ctx: &Ctx, pending: &Pending) -> Result<()> {
    if let Some(pane) = &pending.pane
        && ctx.herdr.pane_get(pane)?.is_some()
    {
        if !pending.close_pane {
            // Delivery to a shared PM may have succeeded before the launcher died.
            // Retain its attached record until a verdict or pane disappearance.
            ctx.store.update(&pending.task, |task| {
                if let Some(review) = task
                    .runs
                    .iter_mut()
                    .find(|r| r.id == pending.run)
                    .and_then(|r| r.review.as_mut())
                    && review.verdict.is_none()
                {
                    review.pane = pending.pane.clone();
                }
                task.review_reservation = None;
                Ok(())
            })?;
            return clear(ctx);
        }
        // Never release a slot if its process could still be running.
        ctx.herdr.pane_close(pane)?;
    }
    ctx.store.update(&pending.task, |task| {
        if let Some(run) = task.runs.iter_mut().find(|r| r.id == pending.run)
            && pending.started
        {
            finish(ctx, run, pending.kind);
        }
        task.review_reservation = None;
        Ok(())
    })?;
    clear(ctx)
}

impl<'a> ReviewLaunch<'a> {
    pub(crate) fn reserve(ctx: &'a Ctx, id: &str, kind: Kind) -> Result<Self> {
        let requested = ctx.store.get(id)?;
        let id = requested.id.as_str();
        let lock = ctx
            .store
            .try_lock("review-budget")?
            .context("another review launch is in progress; try again")?;
        let path = journal(ctx);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let pending = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))?;
                abandon(ctx, &pending)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }

        let mut active = 0;
        for task in ctx.store.list()? {
            for run in &task.runs {
                for (record_kind, pane) in [
                    (
                        Kind::Gate,
                        run.gate
                            .as_ref()
                            .filter(|g| g.holds_slot())
                            .map(|g| g.pane.as_deref()),
                    ),
                    (
                        Kind::Reviewer,
                        run.review
                            .as_ref()
                            .filter(|r| r.verdict.is_none())
                            .map(|r| r.pane.as_deref()),
                    ),
                ] {
                    let Some(pane) = pane else { continue };
                    // Collect completed gates even if their pane remains open.
                    if record_kind == Kind::Gate
                        && task.current_run().is_some_and(|r| r.id == run.id)
                        && crate::gate::gate_collect(ctx, &task.id)?.is_some()
                    {
                        continue;
                    }
                    let alive = match pane {
                        Some(pane) => ctx.herdr.pane_get(pane)?.is_some(),
                        None => false,
                    };
                    if !alive {
                        ctx.store.update(&task.id, |task| {
                            if let Some(run) = task.runs.iter_mut().find(|r| r.id == run.id) {
                                finish(ctx, run, record_kind);
                            }
                            Ok(())
                        })?;
                        continue;
                    }
                    if task.id == id {
                        // Sequential reviewer replacement is supported, but only after its
                        // old pane is closed by review_run under this same lock.
                        if kind == Kind::Reviewer
                            && record_kind == Kind::Reviewer
                            && task.current_run().is_some_and(|r| r.id == run.id)
                            && run.review.as_ref().is_some_and(|r| r.reviewer != "pm")
                        {
                            continue;
                        }
                        if kind == record_kind {
                            bail!("{id}: a gate or reviewer is already running");
                        }
                    }
                    active += 1;
                }
            }
        }
        let task = ctx.store.get(id)?;
        let run = task.current_run().context("task has no run")?;
        let pending = Pending {
            task: task.id.clone(),
            run: run.id.clone(),
            kind,
            pane: None,
            close_pane: true,
            started: false,
        };
        let (_, waiting) = ctx.store.dispatch_update(id, |store, current| {
            if store.config != ctx.store.config {
                bail!("review configuration changed before reservation; retry");
            }
            if current.current_run().map(|r| &r.id) != Some(&run.id)
                || current.status != task.status
            {
                bail!("review run changed before reservation");
            }
            let limit = store.config.limits.review;
            if active >= limit {
                return Ok(Some(format!(
                    "review process limit reached ({active}/{limit})"
                )));
            }
            let counted = match kind {
                Kind::Gate => store.config.limits.count_gates,
                Kind::Reviewer => store.config.limits.count_reviews,
            };
            let replacing = kind == Kind::Reviewer
                && current
                    .current_run()
                    .is_some_and(|r| r.review.as_ref().is_some_and(|r| r.verdict.is_none()));
            if counted
                && let Some(reason) =
                    crate::slots::capacity_reason(store, current, usize::from(replacing))?
            {
                return Ok(Some(reason));
            }
            current.review_reservation = Some(kind);
            current.review_wait = None;
            if let Some(state) = current.current_run_mut().and_then(|r| r.autopilot.as_mut())
                && state
                    .reason
                    .as_ref()
                    .is_some_and(|r| r.starts_with("waiting for a slot"))
            {
                state.reason = None;
            }
            // The journal is written while the same atomic board claim is held.
            save(ctx, &pending)?;
            Ok(None)
        })?;
        if let Some(reason) = waiting {
            return Err(CapacityWait { kind, reason }.into());
        }
        ctx.store.log_event(
            Some(id),
            "review_claimed",
            serde_json::json!({"run": run.id, "kind": kind}),
        )?;
        Ok(Self {
            ctx,
            _lock: lock,
            pending,
            armed: true,
            committed: false,
        })
    }

    pub(crate) fn begin(&mut self) -> Result<()> {
        self.armed = true;
        self.pending.started = true;
        save(self.ctx, &self.pending)
    }

    /// Record the pane before sending any command, closing the crash window.
    pub(crate) fn attach(&mut self, pane: &str, close_pane: bool) -> Result<()> {
        self.pending.pane = Some(pane.into());
        self.pending.close_pane = close_pane;
        self.begin()
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        self.ctx.store.update(&self.pending.task, |task| {
            task.review_reservation = None;
            Ok(())
        })?;
        clear(self.ctx)?;
        self.committed = true;
        Ok(())
    }
}

/// Release completed/gone review processes even when review autopilot is disabled.
pub(crate) fn reconcile(
    ctx: &Ctx,
    panes: &std::collections::HashMap<String, corral_herdr::PaneInfo>,
) -> Result<()> {
    let Some(_lock) = ctx.store.try_lock("review-budget")? else {
        return Ok(());
    };
    match std::fs::read(journal(ctx)) {
        Ok(bytes) => abandon(
            ctx,
            &serde_json::from_slice(&bytes).context("reading pending review launch")?,
        )?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("reading review launch journal"),
    }
    for task in ctx.store.list()? {
        for run in &task.runs {
            for (kind, pane) in [
                (
                    Kind::Gate,
                    run.gate
                        .as_ref()
                        .filter(|g| g.holds_slot())
                        .and_then(|g| g.pane.as_deref()),
                ),
                (
                    Kind::Reviewer,
                    run.review
                        .as_ref()
                        .filter(|r| r.verdict.is_none())
                        .and_then(|r| r.pane.as_deref()),
                ),
            ] {
                if let Some(pane) = pane
                    && !panes.contains_key(pane)
                {
                    ctx.store.update(&task.id, |task| {
                        if let Some(run) = task.runs.iter_mut().find(|r| r.id == run.id) {
                            finish(ctx, run, kind);
                        }
                        Ok(())
                    })?;
                    ctx.store.log_event(
                        Some(&task.id),
                        "review_released",
                        serde_json::json!({"run": run.id, "kind": kind, "reason": "pane gone"}),
                    )?;
                }
            }
        }
    }
    Ok(())
}

impl Drop for ReviewLaunch<'_> {
    fn drop(&mut self) {
        if self.armed
            && !self.committed
            && let Err(error) = abandon(self.ctx, &self.pending)
        {
            crate::messages::report(
                Some(&self.pending.task),
                "review recovery",
                format!("reconciling interrupted review launch: {error:#}"),
            );
        }
    }
}

#[cfg(test)]
#[path = "review_budget_tests.rs"]
mod tests;
