//! `pm act` runs the existing sync executors with a project filter: acting in one project
//! cannot launch, repair, review, land, admit or advance a task of another project.
#[cfg(test)]
#[path = "project_executor_tests.rs"]
mod tests;

use anyhow::{Context, Result};

use super::{PmActionOutcome, PmActionResult, PmExecutable, pm_workset, task_identity};
use crate::Ctx;
use crate::pm_next::PmActionKind;
use crate::sync::SyncScope;

/// `pm act`: recompute the revision-bound action, run the caller's `check` on it before any
/// effect (the native PM action checks its scope there), then execute it. An ID absent from
/// the fresh workset, including any owner-required option, is stale and runs nothing.
pub(crate) fn act(
    ctx: &Ctx,
    id: &str,
    check: impl FnOnce(&PmExecutable) -> Result<()>,
) -> Result<PmActionResult> {
    let current = pm_workset(ctx)?;
    let Some(selected) = current.actions.iter().find(|action| action.id == id) else {
        return Ok(PmActionResult {
            outcome: PmActionOutcome::Stale,
            workset: current,
            diagnosis: None,
        });
    };
    check(selected)?;
    let task = selected
        .action
        .task
        .as_deref()
        .context("automatic action has no task")?;
    if selected.action.kind == PmActionKind::Diagnosis {
        let diagnosis = crate::repair_runtime::repair_diagnose(ctx, task)?;
        return Ok(PmActionResult {
            outcome: PmActionOutcome::Progressed,
            workset: pm_workset(ctx)?,
            diagnosis: Some(diagnosis),
        });
    }
    execute(ctx, task)
}

/// One scoped sync for the revalidated action's task, scoped to the task's stored project (a
/// project-less task reaches only project-less tasks). Progress is the task's identity
/// changing across the run, as for the workspace sync this replaces.
fn execute(ctx: &Ctx, task: &str) -> Result<PmActionResult> {
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
