//! Pane allocation and the actual task transition publishing its cleanup ownership.
use crate::{Ctx, cleanup::WorkspacePublication};
use anyhow::Result;
use corral_core::{HerdrRef, RunHead, TaskRef};

pub(crate) struct Allocation {
    pub herdr: HerdrRef,
    pub publication: Option<WorkspacePublication>,
}

/// Opens a tab in the existing workspace, otherwise allocates and records a new workspace.
pub(crate) fn open_run_pane<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    label: &str,
) -> Result<HerdrRef> {
    Ok(open_run_pane_published(ctx, task, run, label)?.herdr)
}

pub(crate) fn open_run_pane_published<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    label: &str,
) -> Result<Allocation> {
    let workspace = run.herdr.as_ref().map(|h| h.workspace_id.clone());
    if let Some(ws) = workspace
        && ctx.herdr.workspace_list()?.iter().any(|(id, _)| *id == ws)
    {
        let created = ctx.herdr.tab_create(&ws, &run.worktree, label, false)?;
        return Ok(Allocation {
            herdr: HerdrRef {
                workspace_id: created.workspace_id,
                pane_id: created.pane_id,
                socket: Some(ctx.herdr.socket_path()?),
            },
            publication: None,
        });
    }
    let label: String = format!("{} {label}", task.id()).chars().take(40).collect();
    let created = ctx.herdr.workspace_create(&run.worktree, &label, false)?;
    let publication = crate::cleanup::record_run_workspace(
        ctx,
        task.id(),
        &run.id,
        &created.pane_id,
        &created.workspace_id,
    )?;
    Ok(Allocation {
        herdr: HerdrRef {
            workspace_id: created.workspace_id,
            pane_id: created.pane_id,
            socket: Some(ctx.herdr.socket_path()?),
        },
        publication: Some(publication),
    })
}
