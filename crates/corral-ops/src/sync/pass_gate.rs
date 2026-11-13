//! How one sync pass is driven: its fixed inputs, the gate that admits each step, the effect
//! measurement the daemon's incremental planner learns from, and the hot repeat loop.
use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::backend::ChangeSeq;
use corral_core::{Status, TaskHead};
use jiff::Timestamp;

use super::{Change, Effect, FullGate, StepGate, StepId, SyncScope, incremental, sync_scoped};
use crate::feed_delta::FeedDelta;
use crate::{Ctx, ProcessControl};

/// One pass's fixed inputs and the gate that admits its steps.
pub(super) struct PassInputs<'a> {
    pub(super) now: Timestamp,
    pub(super) processes: &'a dyn ProcessControl,
    pub(super) source: Option<&'a Ctx>,
    pub(super) scope: &'a SyncScope,
    gate: &'a mut dyn PassGate,
    /// Iterations begun, and those that reached the end of a pass rather than a fence or an
    /// unknown inventory.
    pub(super) started: u32,
    pub(super) ended: u32,
}

impl<'a> PassInputs<'a> {
    pub(super) fn new(
        now: Timestamp,
        processes: &'a dyn ProcessControl,
        source: Option<&'a Ctx>,
        scope: &'a SyncScope,
        gate: &'a mut dyn PassGate,
    ) -> Self {
        Self {
            now,
            processes,
            source,
            scope,
            gate,
            started: 0,
            ended: 0,
        }
    }

    pub(super) fn gate(&mut self) -> &mut dyn StepGate {
        &mut *self.gate
    }
}

/// A [`StepGate`] that learns when `hot_passes` repeats a pass.
pub(super) trait PassGate: StepGate {
    fn repeat(&mut self, ctx: &Ctx) -> Result<()>;
}

impl PassGate for FullGate {
    fn repeat(&mut self, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }
}

impl PassGate for incremental::Planner<'_> {
    fn repeat(&mut self, ctx: &Ctx) -> Result<()> {
        self.next_iteration(ctx)
    }
}

/// The feed position before a gated step, when `gate` measures effects.
pub(crate) fn mark(ctx: &Ctx, gate: &dyn StepGate) -> Result<Option<ChangeSeq>> {
    if !gate.measures() {
        return Ok(None);
    }
    Ok(Some(ctx.store.view()?.seq()))
}

/// What a step did since `mark`: whether it wrote a non-lease change, the tasks those writes
/// named, and the changes it returned.
pub(crate) fn effect(ctx: &Ctx, mark: Option<ChangeSeq>, changes: usize) -> Result<Effect> {
    let Some(before) = mark else {
        return Ok(Effect {
            wrote: false,
            changes,
            tasks: BTreeSet::new(),
        });
    };
    let delta = {
        let view = ctx.store.view()?;
        FeedDelta::since(view.as_ref(), before, incremental::FEED_LIMIT)
    };
    let tasks = delta
        .tasks
        .iter()
        .map(|num| format!("T-{:03}", num.0))
        .chain(delta.task_ids.iter().cloned())
        .collect();
    Ok(Effect {
        wrote: delta.any(),
        changes,
        tasks,
    })
}

/// A gated step's result; a skipped step yields `Default`.
pub(super) trait StepOutput: Default {
    fn changes(&self) -> usize;
}

impl StepOutput for () {
    fn changes(&self) -> usize {
        0
    }
}

impl StepOutput for Vec<Change> {
    fn changes(&self) -> usize {
        self.len()
    }
}

impl StepOutput for Option<Change> {
    fn changes(&self) -> usize {
        usize::from(self.is_some())
    }
}

/// Runs a global step when `gate` admits it, and reports what it did.
pub(super) fn gated<T: StepOutput>(
    ctx: &Ctx,
    gate: &mut dyn StepGate,
    step: StepId,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if !gate.run(step) {
        return Ok(T::default());
    }
    let before = mark(ctx, gate)?;
    let output = body()?;
    gate.finished(step, effect(ctx, before, output.changes())?);
    Ok(output)
}

/// Runs a per-task step for `head` when `gate` admits it, and reports what it did.
pub(super) fn gated_task<T: StepOutput>(
    ctx: &Ctx,
    gate: &mut dyn StepGate,
    step: StepId,
    head: &TaskHead,
    body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if !gate.task(step, head) {
        return Ok(T::default());
    }
    let before = mark(ctx, gate)?;
    let output = body()?;
    gate.finished_task(step, &head.id, effect(ctx, before, output.changes())?);
    Ok(output)
}

pub(super) fn run_passes(
    ctx: &Ctx,
    pass: &mut PassInputs<'_>,
    attention: Option<&mut Option<crate::attention::AttentionBoard>>,
) -> Result<Vec<Change>> {
    // Latch replacement/schema skew before any preparation reopens a snapshot and
    // reports only a generic incompatible-schema error (T-587).
    ctx.store.check_runtime()?;
    let Some(_loop_pass) = ctx.store.loop_pass()? else {
        return Ok(Vec::new());
    };
    let _pass = corral_core::timing::span("sync.pass");
    if !crate::session_scope::owns_inventory(ctx)? {
        return Ok(Vec::new());
    }
    // Unobserved opens defer PM upgrades until this ownership check has succeeded.
    corral_core::agent::pm_state::migrate(&ctx.store)?;
    // Recovery and derivation preparation run before the measured passes.
    crate::settling::recover(ctx)?;
    let mut rebuild = false;
    let changes = hot_passes(ctx, pass, &mut rebuild, attention)?;
    // Rebuildable state recovers outside the measured hot path (plan T-253 section 4).
    if rebuild && let Err(error) = crate::content_refresh::refresh_search_index(ctx) {
        crate::content_refresh::defer_corrupt_blob_cache_refresh(error)?;
    }
    Ok(changes)
}

/// The measured passes; `rebuild` is set when a pass could not follow the search index.
fn hot_passes(
    ctx: &Ctx,
    pass: &mut PassInputs<'_>,
    rebuild: &mut bool,
    mut attention: Option<&mut Option<crate::attention::AttentionBoard>>,
) -> Result<Vec<Change>> {
    let _hot = corral_core::work::HotScope::enter();
    let mut changes = Vec::new();
    let mut attempted = std::collections::BTreeSet::new();
    loop {
        // The change-feed position, never a serialization of board history.
        let before = ctx.store.view()?.seq();
        let iteration = sync_scoped(ctx, pass, rebuild, attention.as_deref_mut())?;
        let progressed = iteration.iter().any(|change| change.to != Status::NeedsMe);
        changes.extend(iteration);
        let after = ctx.store.view()?.seq();
        // A concurrent owner's write is not this pass's progress. In
        // particular, landing during rebuildable indexing must not make this
        // same sync consume a new repair round on an owner-ready peer.
        if !progressed || before == after {
            break;
        }
        // Existing executors do the work. Recompute only after durable progress,
        // never await an owner question or poll an unchanged external condition.
        let workset = crate::pm_workset(ctx)?;
        if workset.actions.is_empty() {
            break;
        }
        let mut fresh_action = false;
        for action in &workset.actions {
            if let Some(id) = &action.action.task {
                let task = ctx.store.head(id)?;
                // A scoped pass cannot act on a foreign action, so it cannot continue on one.
                if !pass.scope.includes(task.project.as_deref()) {
                    continue;
                }
                // Terminal samples cannot create another attempt. Only a new
                // action, candidate/receipt, plan, round or policy can continue.
                let key = crate::verification::digest(&serde_json::to_vec(&(
                    &workset.policy_revision,
                    id,
                    action.authority,
                    &action.action.evidence,
                    task.rounds,
                    task.current_run
                        .as_ref()
                        .map(|run| (&run.id, &run.gate, &run.review)),
                    task.last_plan
                        .as_ref()
                        .map(|plan| (plan, plan.facts.children_rev)),
                ))?);
                fresh_action |= attempted.insert(key);
            }
        }
        if !fresh_action {
            break;
        }
        // The planner classifies only what was written since this iteration began.
        pass.gate.repeat(ctx)?;
    }
    Ok(changes)
}
