//! A stopped run continues through the existing next-run seed seam. The immutable
//! original package is history, never a reason to rewind newer owned work.
use anyhow::{Context, Result, ensure};
use corral_core::{RunOutcome, Task, TaskRef};
use serde::{Deserialize, Serialize};

use super::{Journal, Package, Prepared, Snapshot, adopt, code_pin, lifecycle};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seed {
    pub selected: code_pin::CodePin,
    /// Integration reference, not its captured commit SHA.
    pub base: String,
    pub snapshot: Snapshot,
}

pub(super) fn snapshot<'a>(journal: &'a Journal, package: &'a Package) -> Option<&'a Snapshot> {
    journal
        .transfer
        .as_ref()
        .and_then(|t| t.continuation_seed.as_ref())
        .map(|s| &s.snapshot)
        .or(package.snapshot.as_ref())
}

pub(super) fn capture(
    ctx: &Ctx,
    task: &Task,
    choice: &super::continuation::Choice,
    digest: &str,
) -> Result<Option<Seed>> {
    let journal = lifecycle::for_task(&ctx.store, task)?.context("transfer missing")?;
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    if transfer.consumed_choices.iter().any(|old| old == digest)
        || lifecycle::plan_only(choice.goal)
    {
        return Ok(None);
    }
    ensure!(
        journal.id == choice.transfer
            && transfer.revision == choice.revision
            && task.contract_revision()? == choice.contract_revision,
        "handoff_stale_choice: task/disposition changed before seed capture"
    );
    if !task
        .current_run()
        .is_some_and(|r| r.outcome == Some(RunOutcome::Cancelled))
    {
        return Ok(None);
    }
    ensure!(
        transfer.disposition == lifecycle::Disposition::Saved
            || transfer.continuation_seed.is_some(),
        "handoff_stop_required: Save and settle the cancelled writer before continuing"
    );
    ensure!(
        code_pin::current(ctx, task)? == choice.code,
        "handoff_stale_choice: stopped code changed; refresh"
    );
    let selected = choice
        .code
        .clone()
        .context("handoff_missing: stopped code is unavailable")?;
    let (path, base_sha, branch, base) = if adopt::pending_seed(ctx, task)? {
        let previous = transfer
            .continuation_seed
            .as_ref()
            .context("handoff_missing: current continuation seed is unrecorded")?;
        let (path, branch) = adopt::seed_path(ctx, task)?;
        (
            path,
            previous.snapshot.state.base.clone(),
            branch,
            previous.base.clone(),
        )
    } else {
        let run = task.current_run().context("stopped run missing")?;
        (
            run.worktree.clone(),
            run.base_sha.clone(),
            run.branch.clone(),
            run.base.clone(),
        )
    };
    let snapshot = Snapshot::capture(&ctx.store, &path, &base_sha, &branch)?;
    ensure!(
        code_pin::of_state(selected.run.clone(), snapshot.state.clone())? == selected,
        "handoff_stale_choice: code changed during checkpoint; refresh"
    );
    Ok(Some(Seed {
        selected,
        base,
        snapshot,
    }))
}

/// No run is fabricated here. Normal dispatch still allocates the one next worker,
/// applies capacity/dependencies and binds its normal execution snapshot.
pub(super) fn adopt(ctx: &Ctx, id: &str, prepared: &Prepared, journal: &Journal) -> Result<bool> {
    let transfer = journal.transfer.as_ref().context("transfer missing")?;
    let Some(seed) = transfer.continuation_seed.as_ref() else {
        return Ok(false);
    };
    ensure!(
        !lifecycle::plan_only(transfer.goal),
        "plan_only: finish the pending seed before changing goals; never materialize code for planning"
    );
    let task = ctx.store.get(id)?;
    let next = format!("R{}", task.runs.len() + 1);
    ensure!(
        transfer.seed_run.as_deref() == Some(next.as_str()),
        "handoff_seed_changed: next-run identity changed"
    );
    ensure!(
        task.current_run()
            .is_some_and(|r| r.outcome == Some(RunOutcome::Cancelled)),
        "handoff_seed_changed: stopped source run changed"
    );
    let (path, branch) = adopt::seed_path(ctx, &task)?;
    adopt::restore_worktree(ctx, &task, &seed.snapshot, &path, &branch)?;
    ctx.store.update(id, |task| {
        ensure!(format!("R{}", task.runs.len() + 1) == next,
            "handoff_seed_changed: run allocation changed during restore");
        let mut current = super::read_journal(&ctx.store, prepared.id())?.context("journal missing")?;
        let transfer = current.transfer.as_mut().context("transfer missing")?;
        ensure!(transfer.continuation_seed.as_ref() == Some(seed)
            && transfer.disposition == lifecycle::Disposition::Starting,
            "handoff_seed_changed: selected source changed during restore");
        transfer.adoption_proven = true;
        transfer.fresh_seed = false;
        transfer.candidate_pending = false; // Stopping never proves the work complete.
        super::recovery::save(&ctx.store, &current)?;
        ctx.store.log_event(Some(id), "handoff_continuation_seeded", serde_json::json!({
            "run":next,"sha":seed.selected.head,"source":seed.selected.run,"original_package_replayed":false
        }))?;
        Ok(())
    })?;
    Ok(true)
}
