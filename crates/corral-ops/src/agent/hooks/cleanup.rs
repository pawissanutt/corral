//! Run-end cleanup seam (integration-seams; recovery reclassifies sessions on discard). A
//! terminal worker run or closed review child loses only its runtime directory; its
//! `.corral/agent/sessions/` stay as a terminal transcript. A lost (resumable) run or an open
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

/// The pi-native containers of `task` that ended and are not yet settled: removable runtime,
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

/// Removes the runtime directory of every ended pi-native container; returns what it removed.
pub fn run_end(ctx: &Ctx, task: &Task) -> Result<Vec<PathBuf>> {
    run_end_ref(ctx, task)
}

/// `run_end` under an explicit runtime root.
pub fn run_end_in(ctx: &Ctx, task: &Task, root: &Path) -> Result<Vec<PathBuf>> {
    run_end_in_ref(ctx, task, root)
}

pub(crate) fn run_end_ref<T: TaskRef + ?Sized>(ctx: &Ctx, task: &T) -> Result<Vec<PathBuf>> {
    run_end_in_ref(ctx, task, &super::super::launch::runtime_root())
}

pub(crate) fn run_end_in_ref<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    root: &Path,
) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for recipient in ended(task)? {
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
