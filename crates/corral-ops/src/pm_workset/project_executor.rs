//! `pm act` runs the existing sync executors with a project filter: acting in one project
//! cannot launch, repair, review, land, admit or advance a task of another project.
#[cfg(test)]
#[path = "project_executor_tests.rs"]
mod tests;

use anyhow::Result;

use super::{PmActionOutcome, PmActionResult, pm_workset, task_identity};
use crate::Ctx;
use crate::sync::SyncScope;

/// One scoped sync for the revalidated action's task, scoped to the task's stored project (a
/// project-less task reaches only project-less tasks). Progress is the task's identity
/// changing across the run, as for the workspace sync this replaces.
pub(super) fn execute(ctx: &Ctx, task: &str) -> Result<PmActionResult> {
    let scope = SyncScope::Project(ctx.store.head(task)?.project.clone());
    let before = task_identity(&ctx.store.get(task)?)?;
    crate::sync::sync_in(ctx, &scope)?;
    let after = task_identity(&ctx.store.get(task)?)?;
    Ok(PmActionResult {
        outcome: if before == after {
            PmActionOutcome::Waiting
        } else {
            PmActionOutcome::Progressed
        },
        workset: pm_workset(ctx)?,
        diagnosis: None,
    })
}
