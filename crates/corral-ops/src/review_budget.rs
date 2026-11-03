//! Review admission is independent of coding slots and dispatch pause.
//! The lock covers admission through launch; the journal covers launcher crashes.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use corral_core::{ActionAuthorization, AutoStage, AutopilotState, GateResult};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use corral_core::{TaskMut as _, TaskRef as _};

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

/// Temporary launch serialization, not a candidate refusal.
#[derive(Debug)]
pub(crate) struct LaunchBusy(pub &'static str);
impl std::fmt::Display for LaunchBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for LaunchBusy {}

impl CapacityWait {
    pub(crate) fn persist(&self, ctx: &Ctx, id: &str, now: Timestamp) -> Result<()> {
        ctx.store.update_head(id, |task| {
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
    /// The agent request whose submission the store refused; evidence only, never authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused_request: Option<String>,
}

/// The corral-pi request a gate launch executes for; `ReviewLaunch::submit` fences on it.
#[derive(Debug, Clone)]
pub(crate) struct GateRequest {
    pub(crate) request_id: String,
    pub(crate) attempt: String,
    pub(crate) binding: corral_core::agent::RequestBinding,
}

pub(crate) struct ReviewLaunch<'a> {
    ctx: &'a Ctx,
    _lock: corral_core::StoreLock,
    pending: Pending,
    armed: bool,
    committed: bool,
    request: Option<GateRequest>,
}

fn save(ctx: &Ctx, pending: &Pending) -> Result<()> {
    ctx.store.save_record("review-launch", "current", pending)
}

fn load_pending(ctx: &Ctx) -> Result<Option<Pending>> {
    ctx.store.record("review-launch", "current")
}

fn clear(ctx: &Ctx) -> Result<()> {
    ctx.store.delete_record("review-launch", "current")
}

/// Ends the current run's slot-holding gate or reviewer as lost: the reviewer snapshot is its
/// current child.
pub(crate) fn finish_head(
    task: &mut corral_core::TaskHeadMut,
    kind: Kind,
    now: Timestamp,
) -> Result<()> {
    let current = task.current_run_mut().context("no current run")?;
    match kind {
        Kind::Gate => {
            if let Some(gate) = &mut current.gate
                && gate.result == GateResult::Running
            {
                gate.process_lost = true;
            }
        }
        Kind::Reviewer => {
            let Some(review) = current.review.as_mut().filter(|r| r.holds_slot()) else {
                return Ok(());
            };
            review.process_lost = true;
            let review = review.clone();
            if current
                .current_child
                .as_deref()
                .is_some_and(|child| child.ended.is_none())
            {
                // The finish is the child's projection; the next usage refresh collects its
                // usage (`usage_pending`), so no nested row of its history is read here.
                if let Some(child) = task
                    .current_run_mut()
                    .and_then(|r| r.current_child.as_deref_mut())
                {
                    child.review = Some(review);
                    child.ended = Some(now);
                    child.usage_pending = true;
                }
            }
        }
    }
    Ok(())
}

/// The gate and reviewer processes holding review slots, besides the one `id` is replacing.
/// Slots are held only by open tasks' current runs: each is read from its head, with its
/// current review child.
fn active_processes(ctx: &Ctx, id: &str, kind: Kind) -> Result<usize> {
    let mut active = 0;
    for head in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        let Some(run) = head.current_run.as_deref() else {
            continue;
        };
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
                && let Some(_storage) = crate::artifact_storage::try_storage_lock(ctx)?
                && crate::gate::gate_collect(ctx, &head.id)?.is_some()
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
                        task: (&head).into(),
                        pane,
                        started,
                        seen: false,
                        present: pane
                            .is_some_and(|id| inventory.panes.iter().any(|p| p.pane_id == id)),
                        at: corral_core::clock::now(),
                        ticket: Some(inventory.ticket),
                        count_miss: false,
                    },
                )?,
                None => false,
            };
            if gone {
                let released = ctx.store.try_update_head(&head.id, |task| {
                    if !crate::session_scope::owns_head(ctx, task.head())? {
                        return Ok(false);
                    }
                    if !task.head().current_run.as_ref().is_some_and(|current| {
                        current.id == run.id && same_process_heads(current, run, record_kind)
                    }) {
                        return Ok(false);
                    }
                    finish_head(task, record_kind, ctx.store.now())?;
                    Ok(true)
                })?;
                if released.is_some() {
                    continue;
                }
            }
            if head.id == id {
                // Legacy gates cannot be collected. gate_start closes their
                // old pane before replacing them under this same reservation.
                if kind == Kind::Gate
                    && record_kind == Kind::Gate
                    && run.gate.as_ref().is_some_and(|g| g.identity.is_none())
                {
                    continue;
                }
                // Sequential reviewer replacement is supported, but only after its
                // old pane is closed by review_run under this same lock.
                if kind == Kind::Reviewer
                    && record_kind == Kind::Reviewer
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
    Ok(active)
}

fn same_process_heads(
    current: &corral_core::RunHead,
    observed: &corral_core::RunHead,
    kind: Kind,
) -> bool {
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
            ctx.store.update_head(&pending.task, |task| {
                if task.run_head(&pending.run)?.is_some() {
                    task.edit_run(&pending.run, |run| {
                        if let Some(review) = run.review.as_mut()
                            && review.verdict.is_none()
                        {
                            review.pane = pending.pane.clone();
                        }
                        Ok(())
                    })?;
                }
                task.review_reservation = None;
                Ok(())
            })?;
            return clear(ctx);
        }
        // Never release a slot if its process could still be running.
        ctx.herdr.pane_close(pane)?;
    }
    ctx.store.update_head(&pending.task, |task| {
        if pending.started
            && task
                .current_run_head()?
                .is_some_and(|run| run.id == pending.run)
        {
            finish_head(task, pending.kind, ctx.store.now())?;
        }
        task.review_reservation = None;
        Ok(())
    })?;
    clear(ctx)
}

/// Advisory capacity probe before rewriting a co-work candidate that is already waiting for
/// a reviewer. Unlike a launch reservation this never consumes/abandons a prepared attempt;
/// the actual launch still rechecks all admission under its own lease.
pub(crate) fn reviewer_waiting(ctx: &Ctx, id: &str) -> Result<bool> {
    let Some(_lock) = ctx.store.try_lock("review-budget")? else {
        return Ok(true);
    };
    let task = ctx.store.head(id)?;
    let active = active_processes(ctx, id, Kind::Reviewer)?;
    if crate::resources::admission(ctx)?.is_some() || active >= ctx.store.config.limits.review {
        return Ok(true);
    }
    let replacing = task.current_run.as_ref().is_some_and(|run| {
        run.review
            .as_ref()
            .is_some_and(corral_core::ReviewRecord::holds_slot)
    });
    Ok(ctx.store.config.limits.count_reviews
        && crate::slots::capacity_reason(&ctx.store, &task, usize::from(replacing))?.is_some())
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
        let requested = ctx.store.head(id)?;
        corral_core::Park::require_unparked_ref(&requested)?;
        let id = requested.id.as_str();
        let lock = ctx.store.try_lock("review-budget")?.ok_or(LaunchBusy(
            "another review launch is in progress; try again",
        ))?;
        crate::session_scope::check_launch(ctx)?;
        if let Some(pending) = load_pending(ctx)? {
            abandon(ctx, &pending)?;
        }
        let active = active_processes(ctx, id, kind)?;
        let task = ctx.store.head(id)?;
        let run = task.current_run.as_deref().context("task has no run")?;
        crate::operating::require_origin(&ctx.store, &task, submission::action_kind(kind), origin)?;
        let mut retained = submission::load_retained(ctx, &task.id, &run.id, kind)?;
        if let Some(pending) = &retained
            && recovery::stale(ctx, pending)?
        {
            recovery::supersede(ctx, pending)?;
            retained = None;
        }
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
                refused_request: None,
            }
        };
        let ownership = ctx.store.try_lock("session-ownership")?.ok_or(LaunchBusy(
            "pane inventory is being reconciled; retry review launch",
        ))?;
        let (_, waiting) = ctx.store.dispatch_update_head(id, |store, current| {
            corral_core::Park::require_unparked_ref(&*current)?;
            crate::operating::require_origin(
                store,
                &*current,
                submission::action_kind(kind),
                origin,
            )?;
            crate::session_scope::check_launch_in(ctx, store)?;
            // Only edits that change this task's verification inputs (its effective
            // policy, including the reviewer rules select, or reviewer execution
            // settings) invalidate what was prepared under the earlier snapshot.
            if !crate::verification::same_task_inputs(&ctx.store.config, &store.config, &*current)?
            {
                bail!("review configuration changed before reservation; retry");
            }
            if current.current_run_head()?.map(|r| r.id.clone()).as_ref() != Some(&run.id)
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
            if kind == Kind::Gate
                && let Some(reason) =
                    crate::parked::gate_precedence(store, &current.shell_task()?, active)?
            {
                return Ok(Some(reason));
            }
            let counted = match kind {
                Kind::Gate => store.config.limits.count_gates,
                Kind::Reviewer => store.config.limits.count_reviews,
            };
            let replacing = current.current_run_head()?.is_some_and(|r| match kind {
                Kind::Reviewer => r
                    .review
                    .as_ref()
                    .is_some_and(corral_core::ReviewRecord::holds_slot),
                Kind::Gate => r
                    .gate
                    .as_ref()
                    .is_some_and(|g| g.holds_slot() && g.identity.is_none()),
            });
            let sharing = kind == Kind::Gate
                && current.status.holds_slot()
                && current.head().holds_dispatch_slot()
                && current.head().current_run.is_some();
            if counted
                && let Some(reason) = crate::slots::capacity_reason(
                    store,
                    &*current,
                    usize::from(replacing || sharing),
                )?
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
                        .current_child
                        .as_deref_mut()
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
            ctx.store.log_event(
                Some(id),
                "review_claimed",
                serde_json::json!({"run": run.id, "kind": kind}),
            )?;
            Ok(None)
        })?;
        drop(ownership);
        if let Some(reason) = waiting {
            return Err(CapacityWait { kind, reason }.into());
        }
        Ok(Self {
            ctx,
            _lock: lock,
            pending,
            armed: true,
            committed: false,
            request: None,
        })
    }

    /// Binds the agent request this launch executes, before either submit path runs.
    pub(crate) fn bind_request(&mut self, request: GateRequest) {
        self.request = Some(request);
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
        self.ctx.store.update_head(&self.pending.task, |task| {
            task.review_reservation = None;
            Ok(())
        })?;
        clear(self.ctx)?;
        recovery::clear_retained(self.ctx, &self.pending)?;
        self.committed = true;
        Ok(())
    }
}

pub(crate) fn retained_preparation(
    store: &corral_core::Store,
    task: &str,
    run: &str,
    kind: Kind,
) -> Result<bool> {
    submission::has_retained(store, task, run, kind)
}

mod attempt;
mod recovery;

#[cfg(test)]
pub(crate) use attempt::{AttemptState, attempt_state, retained_attempt};
pub use attempt::{DueWake, due_gate_wakes, fire_wake, own_attempt};
pub(crate) use attempt::{adoptable_attempt, bounded, due_gate_wakes_ref, evidence, settle};

/// Release completed/gone review processes even when review autopilot is disabled.
pub(crate) fn reconcile(
    ctx: &Ctx,
    snapshot: crate::pane_liveness::Snapshot<'_>,
    now: Timestamp,
) -> Result<()> {
    let panes = snapshot.panes;
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
    if let Some(pending) = load_pending(ctx)? {
        abandon(ctx, &pending)?;
    }
    // Gate and reviewer slots are held only by open tasks' current runs; the shell holds that
    // run and its current review child.
    for head in ctx
        .store
        .heads(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        if !crate::session_scope::owns_head(ctx, &head)? {
            continue;
        }
        if let Some(run) = head.current_run.as_deref() {
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
                let exited = kind == Kind::Reviewer
                    && run.current_child.as_deref().is_some_and(|child| {
                        let attempt = format!("{}-{}", run.id, child.id);
                        crate::launch_exit::exited_for(ctx, &head.id, &attempt)
                    });
                if exited
                    || crate::pane_liveness::gone(
                        ctx,
                        crate::pane_liveness::Sample {
                            task: (&head).into(),
                            pane,
                            started,
                            seen: false,
                            present: pane.is_some_and(|id| panes.contains_key(id)),
                            at: now,
                            ticket: snapshot.ticket,
                            count_miss: true,
                        },
                    )?
                {
                    let mut release_reason = None;
                    let released = ctx.store.try_update_head(&head.id, |task| {
                        if !crate::session_scope::owns_head(ctx, task.head())? {
                            return Ok(false);
                        }
                        if !task
                            .head()
                            .current_run
                            .as_ref()
                            .is_some_and(|r| r.id == run.id && same_process_heads(r, run, kind))
                        {
                            return Ok(false);
                        }
                        finish_head(task, kind, ctx.store.now())?;
                        let reason = if exited {
                            crate::launch_exit::reason(ctx, "The reviewer", pane)
                        } else {
                            "pane gone".into()
                        };
                        ctx.store.log_event(
                            Some(&task.id),
                            "review_released",
                            serde_json::json!({"run": run.id, "kind": kind, "reason": reason}),
                        )?;
                        release_reason = exited.then_some(reason);
                        Ok(true)
                    })?;
                    if let Some(reason) = release_reason {
                        crate::launch_exit::record(
                            ctx,
                            &head.id,
                            &run.id,
                            "Reviewer launch failure",
                            &reason,
                        )?;
                    } else if released.is_some()
                        && kind == Kind::Reviewer
                        && let Some(child) = run.current_child.as_deref()
                    {
                        let attempt = format!("{}-{}", run.id, child.id);
                        crate::launch_exit::record_missing(&crate::launch_exit::receipt(
                            ctx, &head.id, &attempt,
                        )?)?;
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
