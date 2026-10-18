//! Review admission is independent of coding slots and dispatch pause.
//! The lock covers admission through launch; the journal covers launcher crashes.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use corral_core::{ActionAuthorization, AutoStage, AutopilotState, GateResult, Run};
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
        if self.reason.starts_with("waiting: machine busy") {
            f.write_str(&self.reason)
        } else {
            write!(f, "waiting for a slot: {}", self.reason)
        }
    }
}
impl std::error::Error for CapacityWait {}

impl CapacityWait {
    pub(crate) fn persist(&self, ctx: &Ctx, id: &str, now: Timestamp) -> Result<()> {
        ctx.store.update(id, |task| {
            task.review_wait = Some(self.kind);
            if let Some(run) = task.current_run_mut() {
                let state = run.autopilot.get_or_insert(AutopilotState {
                    stage: match self.kind {
                        Kind::Gate => AutoStage::Gating,
                        Kind::Reviewer => AutoStage::Reviewing,
                    },
                    reason: None,
                    sha: None,
                    at: now,
                });
                state.reason = Some(self.to_string());
            }
            Ok(())
        })?;
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    task: String,
    run: String,
    kind: Kind,
    #[serde(default)]
    socket: Option<PathBuf>,
    pane: Option<String>,
    // A PM pane belongs to the workspace, not this review.
    close_pane: bool,
    // Legacy journals used this for uncertainty; new journals separately record submission.
    #[serde(default)]
    started: bool,
    // None is legacy evidence, never implicit owner authority.
    #[serde(default)]
    automatic: Option<bool>,
    #[serde(default)]
    authorization: Option<ActionAuthorization>,
    #[serde(default)]
    submitted: Option<bool>,
    #[serde(default)]
    review_run: Option<String>,
    #[serde(default)]
    refused: Option<String>,
    #[serde(default)]
    prepared_gate: Option<corral_core::GateRecord>,
    #[serde(default)]
    prepared_review: Option<corral_core::ReviewRecord>,
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

pub(crate) fn finish(ctx: &Ctx, run: &mut Run, kind: Kind) {
    match kind {
        Kind::Gate => {
            if let Some(gate) = &mut run.gate
                && gate.result == GateResult::Running
            {
                gate.process_lost = true;
            }
        }
        Kind::Reviewer => {
            if let Some(review) = run.review.as_mut().filter(|r| r.holds_slot()) {
                review.process_lost = true;
                if let Some(snapshot) = run.review_runs.last_mut()
                    && snapshot.ended.is_none()
                {
                    snapshot.review = Some(review.clone());
                    snapshot.ended = Some(Timestamp::now());
                    crate::usage::collect(snapshot, &ctx.store);
                }
            }
        }
    }
}

fn same_process(current: &Run, observed: &Run, kind: Kind) -> bool {
    match kind {
        Kind::Gate => current.gate == observed.gate,
        Kind::Reviewer => current.review == observed.review,
    }
}

fn abandon(ctx: &Ctx, pending: &Pending) -> Result<()> {
    if pending.submitted == Some(false) && pending.started {
        return submission::retain(ctx, pending);
    }
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
    #[cfg(test)]
    pub(crate) fn reserve(ctx: &'a Ctx, id: &str, kind: Kind) -> Result<Self> {
        Self::reserve_origin(ctx, id, kind, false, crate::operating::Origin::Owner)
    }

    pub(crate) fn reserve_origin(
        ctx: &'a Ctx,
        id: &str,
        kind: Kind,
        force_resources: bool,
        origin: crate::operating::Origin,
    ) -> Result<Self> {
        crate::session_scope::check_launch(ctx)?;
        let requested = ctx.store.get(id)?;
        let id = requested.id.as_str();
        let lock = ctx
            .store
            .try_lock("review-budget")?
            .context("another review launch is in progress; try again")?;
        crate::session_scope::check_launch(ctx)?;
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
                            .filter(|r| r.holds_slot())
                            .map(|r| r.pane.as_deref()),
                    ),
                ] {
                    let Some(pane) = pane else { continue };
                    // Collect completed gates even if their pane remains open.
                    if record_kind == Kind::Gate
                        && run.gate.as_ref().is_some_and(|g| g.identity.is_some())
                        && task.current_run().is_some_and(|r| r.id == run.id)
                        && crate::gate::gate_collect(ctx, &task.id)?.is_some()
                    {
                        continue;
                    }
                    let inventory = crate::pane_liveness::inventory(ctx);
                    let started = match record_kind {
                        Kind::Gate => run.gate.as_ref().map(|g| g.at),
                        Kind::Reviewer => run.review.as_ref().map(|r| r.at),
                    }
                    .context("active review process has no start time")?;
                    let gone = match inventory {
                        Some(inventory) => crate::pane_liveness::gone(
                            ctx,
                            crate::pane_liveness::Sample {
                                task: &task,
                                pane,
                                started,
                                seen: false,
                                present: pane.is_some_and(|id| {
                                    inventory.panes.iter().any(|p| p.pane_id == id)
                                }),
                                at: Timestamp::now(),
                                count_miss: false,
                            },
                        )?,
                        None => false,
                    };
                    if gone {
                        let released = ctx.store.try_update(&task.id, |task| {
                            if !crate::session_scope::owns_task(ctx, task)? {
                                return Ok(false);
                            }
                            let Some(current) = task
                                .runs
                                .iter_mut()
                                .find(|r| r.id == run.id && same_process(r, run, record_kind))
                            else {
                                return Ok(false);
                            };
                            finish(ctx, current, record_kind);
                            Ok(true)
                        })?;
                        if released.is_some() {
                            continue;
                        }
                    }
                    if task.id == id {
                        // Legacy gates cannot be collected. gate_start closes their
                        // old pane before replacing them under this same reservation.
                        if kind == Kind::Gate
                            && record_kind == Kind::Gate
                            && task.current_run().is_some_and(|r| r.id == run.id)
                            && run.gate.as_ref().is_some_and(|g| g.identity.is_none())
                        {
                            continue;
                        }
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
        crate::operating::require_origin(&ctx.store, &task, submission::action_kind(kind), origin)?;
        let retained = submission::load_retained(ctx, &task.id, &run.id, kind)?;
        let pending = if let Some(pending) = retained {
            recovery::prepare(ctx, pending, origin)?
        } else {
            Pending {
                task: task.id.clone(),
                run: run.id.clone(),
                kind,
                socket: Some(ctx.herdr.socket_path()?),
                pane: None,
                close_pane: true,
                started: false,
                automatic: Some(matches!(origin, crate::operating::Origin::Automatic)),
                authorization: None,
                submitted: Some(false),
                review_run: None,
                refused: None,
                prepared_gate: None,
                prepared_review: None,
            }
        };
        let ownership = ctx
            .store
            .try_lock("session-ownership")?
            .context("pane inventory is being reconciled; retry review launch")?;
        let (_, waiting) = ctx.store.dispatch_update(id, |store, current| {
            crate::operating::require_origin(
                store,
                current,
                submission::action_kind(kind),
                origin,
            )?;
            crate::session_scope::check_launch_in(ctx, store)?;
            if store.config != ctx.store.config {
                bail!("review configuration changed before reservation; retry");
            }
            if current.current_run().map(|r| &r.id) != Some(&run.id)
                || current.status != task.status
            {
                bail!("review run changed before reservation");
            }
            if !force_resources && let Some(reason) = crate::resources::admission(ctx)? {
                return Ok(Some(reason));
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
            let replacing = current.current_run().is_some_and(|r| match kind {
                Kind::Reviewer => r
                    .review
                    .as_ref()
                    .is_some_and(corral_core::ReviewRecord::holds_slot),
                Kind::Gate => r
                    .gate
                    .as_ref()
                    .is_some_and(|g| g.holds_slot() && g.identity.is_none()),
            });
            if counted
                && let Some(reason) =
                    crate::slots::capacity_reason(store, current, usize::from(replacing))?
            {
                return Ok(Some(reason));
            }
            current.review_reservation = Some(kind);
            current.review_wait = None;
            if let Some(run) = current.current_run_mut() {
                if let Some(gate) = &pending.prepared_gate {
                    run.gate = Some(gate.clone());
                }
                if let Some(review) = &pending.prepared_review {
                    run.review = Some(review.clone());
                    let child = run
                        .review_runs
                        .last_mut()
                        .filter(|child| Some(&child.id) == pending.review_run.as_ref())
                        .context("prepared reviewer child changed")?;
                    child.ended = None;
                }
            }
            if let Some(state) = current.current_run_mut().and_then(|r| r.autopilot.as_mut())
                && state.reason.as_ref().is_some_and(|r| {
                    r.starts_with("waiting for a slot") || r.starts_with("waiting: machine busy")
                })
            {
                state.reason = None;
            }
            // The journal is written while the same atomic board claim is held.
            save(ctx, &pending)?;
            Ok(None)
        })?;
        drop(ownership);
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
        save(self.ctx, &self.pending)
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        self.ctx.store.update(&self.pending.task, |task| {
            task.review_reservation = None;
            Ok(())
        })?;
        clear(self.ctx)?;
        recovery::clear_retained(self.ctx, &self.pending)?;
        self.committed = true;
        Ok(())
    }
}

mod recovery;

/// Release completed/gone review processes even when review autopilot is disabled.
pub(crate) fn reconcile(
    ctx: &Ctx,
    panes: &std::collections::HashMap<String, corral_herdr::PaneInfo>,
    now: Timestamp,
) -> Result<()> {
    if panes.is_empty() {
        crate::pane_liveness::unknown("herdr returned an empty pane inventory");
        return Ok(());
    }
    let Some(_lock) = ctx.store.try_lock("review-budget")? else {
        return Ok(());
    };
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(());
    }
    match std::fs::read(journal(ctx)) {
        Ok(bytes) => abandon(
            ctx,
            &serde_json::from_slice(&bytes).context("reading pending review launch")?,
        )?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("reading review launch journal"),
    }
    for task in ctx.store.list()? {
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        for run in &task.runs {
            for (kind, record) in [
                (
                    Kind::Gate,
                    run.gate
                        .as_ref()
                        .filter(|g| g.holds_slot())
                        .map(|g| (g.pane.as_deref(), g.at)),
                ),
                (
                    Kind::Reviewer,
                    run.review
                        .as_ref()
                        .filter(|r| r.holds_slot())
                        .map(|r| (r.pane.as_deref(), r.at)),
                ),
            ] {
                let Some((pane, started)) = record else {
                    continue;
                };
                if crate::pane_liveness::gone(
                    ctx,
                    crate::pane_liveness::Sample {
                        task: &task,
                        pane,
                        started,
                        seen: false,
                        present: pane.is_some_and(|id| panes.contains_key(id)),
                        at: now,
                        count_miss: true,
                    },
                )? {
                    let released = ctx.store.try_update(&task.id, |task| {
                        if !crate::session_scope::owns_task(ctx, task)? {
                            return Ok(false);
                        }
                        let Some(current) = task
                            .runs
                            .iter_mut()
                            .find(|r| r.id == run.id && same_process(r, run, kind))
                        else {
                            return Ok(false);
                        };
                        finish(ctx, current, kind);
                        Ok(true)
                    })?;
                    if released.is_some() {
                        ctx.store.log_event(
                            Some(&task.id),
                            "review_released",
                            serde_json::json!({"run": run.id, "kind": kind, "reason": "pane gone"}),
                        )?;
                    }
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

mod submission;
