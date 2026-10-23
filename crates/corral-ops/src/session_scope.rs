//! A store's pane IDs are meaningful only on their launch server.

use std::path::PathBuf;

use anyhow::{Result, bail};
use corral_core::work::{Work, record};
use corral_core::{HerdrRef, HerdrSessionBinding, Run, Store, Task};

use crate::Ctx;

fn run_refs<'a>(run: &'a Run, refs: &mut Vec<&'a HerdrRef>) {
    refs.extend(run.herdr.iter());
    for child in &run.review_runs {
        run_refs(child, refs);
    }
}

/// Fail closed for mixed ownership, including legacy refs. This check is read-only
/// and deliberately silent: plugin events can invoke foreign hooks many times a second.
/// Within one pass the verdict is computed once per shared task-list snapshot.
pub(crate) fn owns_inventory(ctx: &Ctx) -> Result<bool> {
    let tasks = ctx.store.list_shared()?;
    let view = ctx.store.session_view()?;
    view.verdict(&tasks, ctx.herdr.session(), || {
        record(Work::OwnershipScan, 1);
        owns_with(ctx, view.binding.as_ref(), view.pending.as_ref(), &tasks)
    })
}

/// Check the snapshots actually being consumed, not a task list read before IPC.
/// The binding comes from the current pass's single read.
pub(crate) fn owns_tasks(ctx: &Ctx, store: &Store, tasks: &[Task]) -> Result<bool> {
    record(Work::OwnershipScan, 1);
    let view = store.session_view()?;
    owns_with(ctx, view.binding.as_ref(), view.pending.as_ref(), tasks)
}

fn owns_with(
    ctx: &Ctx,
    binding: Option<&HerdrSessionBinding>,
    pending: Option<&Option<PathBuf>>,
    tasks: &[Task],
) -> Result<bool> {
    let mut refs = Vec::new();
    for task in tasks {
        for run in &task.runs {
            run_refs(run, &mut refs);
        }
        refs.extend(task.plan_runs.iter().filter_map(|r| r.herdr.as_ref()));
        refs.extend(
            task.plans
                .iter()
                .flat_map(|p| &p.reviews)
                .filter_map(|r| r.herdr.as_ref()),
        );
        refs.extend(
            task.aggregate_attempts
                .iter()
                .flat_map(|a| &a.reviews)
                .filter_map(|r| r.herdr.as_ref()),
        );
    }
    let launches: Vec<_> = tasks
        .iter()
        .filter_map(|task| task.launch.as_ref())
        .collect();
    let gates: Vec<_> = tasks
        .iter()
        .flat_map(|task| &task.runs)
        .flat_map(gate_sockets)
        .collect();
    if refs.is_empty()
        && launches.is_empty()
        && gates.is_empty()
        && pending.is_none()
        && binding.is_none()
    {
        // There are no server-scoped pane IDs or reserved launches to reconcile yet.
        return Ok(true);
    }
    let actual = ctx.herdr.socket_path()?;
    let legacy = binding.map(|binding| &binding.socket);
    Ok(legacy.is_none_or(|socket| socket == &actual)
        && refs
            .into_iter()
            .all(|h| h.socket.as_ref().or(legacy) == Some(&actual))
        && launches
            .into_iter()
            .all(|launch| launch.socket.as_ref().or(legacy) == Some(&actual))
        && gates
            .into_iter()
            .all(|socket| socket.or(legacy) == Some(&actual))
        && pending.is_none_or(|socket| socket.as_ref().or(legacy) == Some(&actual)))
}

fn gate_sockets(run: &Run) -> Vec<Option<&std::path::PathBuf>> {
    let mut sockets = Vec::new();
    if let Some(gate) = &run.gate
        && gate.pane.is_some()
    {
        sockets.push(gate.socket.as_ref());
    }
    for child in &run.review_runs {
        sockets.extend(gate_sockets(child));
    }
    sockets
}

/// One task's check, using the pass's binding read; not a board scan.
pub(crate) fn owns_task(ctx: &Ctx, task: &Task) -> Result<bool> {
    let view = ctx.store.session_view()?;
    owns_with(
        ctx,
        view.binding.as_ref(),
        view.pending.as_ref(),
        std::slice::from_ref(task),
    )
}

/// New launches may bind an empty store, but may not mix existing pane owners.
pub(crate) fn check_launch(ctx: &Ctx) -> Result<()> {
    check_launch_in(ctx, &ctx.store)
}

/// Called under the board writer lock before publishing any launch claim. It reads the
/// binding afresh, never a pass's earlier read.
pub(crate) fn check_launch_in(ctx: &Ctx, store: &Store) -> Result<()> {
    record(Work::OwnershipScan, 1);
    let view = store.read_session_view()?;
    let tasks = store.list_shared()?;
    if !owns_with(ctx, view.binding.as_ref(), view.pending.as_ref(), &tasks)? {
        bail!(
            "selected herdr session does not own this store's panes; unbound legacy panes require `corral bind-session --socket <launch-socket> --launch-record <evidence-file>`"
        );
    }
    Ok(())
}
