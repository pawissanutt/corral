//! A store's pane IDs are meaningful only on their launch server.

use anyhow::{Result, bail};
use corral_core::{HerdrRef, Run};

use crate::Ctx;

fn run_refs<'a>(run: &'a Run, refs: &mut Vec<&'a HerdrRef>) {
    refs.extend(run.herdr.iter());
    for child in &run.review_runs {
        run_refs(child, refs);
    }
}

/// Fail closed for mixed ownership, including legacy refs. This check is read-only
/// and deliberately silent: plugin events can invoke foreign hooks many times a second.
pub(crate) fn owns_inventory(ctx: &Ctx) -> Result<bool> {
    let tasks = ctx.store.list()?;
    let mut refs = Vec::new();
    for task in &tasks {
        for run in &task.runs {
            run_refs(run, &mut refs);
        }
        refs.extend(
            task.plans
                .iter()
                .flat_map(|p| &p.reviews)
                .filter_map(|r| r.herdr.as_ref()),
        );
    }
    let launches: Vec<_> = tasks
        .iter()
        .filter_map(|task| task.launch.as_ref())
        .collect();
    let binding = ctx.store.herdr_session_binding()?;
    if refs.is_empty() && launches.is_empty() && binding.is_none() {
        // There are no server-scoped pane IDs or reserved launches to reconcile yet.
        return Ok(true);
    }
    let actual = ctx.herdr.socket_path()?;
    let legacy = binding.as_ref().map(|binding| &binding.socket);
    Ok(legacy.is_none_or(|socket| socket == &actual)
        && refs
            .into_iter()
            .all(|h| h.socket.as_ref().or(legacy) == Some(&actual))
        && launches
            .into_iter()
            .all(|launch| launch.socket.as_ref().or(legacy) == Some(&actual)))
}

/// New launches may bind an empty store, but may not mix existing pane owners.
pub(crate) fn check_launch(ctx: &Ctx) -> Result<()> {
    if !owns_inventory(ctx)? {
        bail!(
            "selected herdr session does not own this store's panes; unbound legacy panes require `corral bind-session --socket <launch-socket> --launch-record <evidence-file>`"
        );
    }
    Ok(())
}
