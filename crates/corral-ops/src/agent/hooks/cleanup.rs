//! Run-end cleanup seam (integration-seams; recovery reclassifies sessions on discard). A
//! terminal worker run or closed review child loses only its runtime directory; its
//! `.corral/agent/sessions/` stay, promoted to owned blob-backed transcripts first. A lost
//! (resumable) run or an open
//! review child keeps both.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{ContainerRef, container_closure_ref};
use corral_core::inbox::Recipient;
use corral_core::{Status, Task, TaskRef};

use super::consulted;
use crate::Ctx;

#[cfg(test)]
#[path = "cleanup_head_tests.rs"]
mod head_tests;

/// The corral-pi containers of `task` that ended and are not yet settled: removable runtime,
/// kept sessions. A settled container never costs another filesystem call.
fn ended<T: TaskRef + ?Sized>(task: &T) -> Result<Vec<Recipient>> {
    let terminal = matches!(task.status(), Status::Done | Status::Dropped);
    let mut out = Vec::new();
    for container in task.unfinished_native_containers()? {
        if !matches!(
            container,
            ContainerRef::Worker { .. } | ContainerRef::Reviewer { .. }
        ) {
            continue;
        }
        let recipient = crate::questions::recipient(&container)
            .context("coding container recipient missing")?;
        let inbox = task
            .inbox_head(&recipient)?
            .context("native cleanup inbox missing")?;
        if inbox.settled.is_none()
            && (terminal || container_closure_ref(task, &container)?.is_some())
        {
            out.push(recipient);
        }
    }
    Ok(out)
}

/// Promotes ended containers' transcripts, then removes ended Pi runtimes (not sessions).
pub fn run_end(ctx: &Ctx, task: &Task) -> Result<Vec<PathBuf>> {
    run_end_in(ctx, task, &super::super::launch::runtime_root())
}

/// `run_end` under an explicit runtime root.
pub fn run_end_in(ctx: &Ctx, task: &Task, root: &Path) -> Result<Vec<PathBuf>> {
    crate::transcripts::run_end(ctx, task)?;
    remove_ended(ctx, ended(task)?, root)
}

pub(crate) fn run_end_ref<T: TaskRef + ?Sized>(ctx: &Ctx, task: &T) -> Result<Vec<PathBuf>> {
    run_end_in_ref(ctx, task, &super::super::launch::runtime_root())
}

pub(crate) fn run_end_in_ref<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    root: &Path,
) -> Result<Vec<PathBuf>> {
    let recipients = ended(task)?;
    for recipient in &recipients {
        let owner = match recipient {
            Recipient::Run { task, run } => ContainerRef::Worker {
                task: task.clone(),
                run: run.clone(),
            },
            Recipient::ReviewRun { task, run, review } => ContainerRef::Reviewer {
                task: task.clone(),
                run: run.clone(),
                review: review.clone(),
            },
            _ => anyhow::bail!("native cleanup recipient is not a coding container"),
        };
        crate::transcripts::sessions(ctx, owner)?;
    }
    remove_ended(ctx, recipients, root)
}

fn remove_ended(ctx: &Ctx, recipients: Vec<Recipient>, root: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for recipient in recipients {
        consulted("cleanup");
        let dir = super::super::launch::runtime_dir_in(root, &ctx.store, &recipient)?;
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => removed.push(dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", dir.display()));
            }
        }
    }
    Ok(removed)
}
