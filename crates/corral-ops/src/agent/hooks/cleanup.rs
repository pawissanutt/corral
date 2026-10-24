//! Run-end cleanup seam (integration-seams; recovery reclassifies sessions on discard). A
//! terminal worker run or closed review child loses only its runtime directory; its
//! `.corral/agent/sessions/` stay as a terminal transcript. A lost (resumable) run or an open
//! review child keeps both.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{ContainerRef, container_closure};
use corral_core::inbox::Recipient;
use corral_core::{Status, Task};

use super::{consulted, run_is_pi_native};
use crate::Ctx;

/// The pi-native containers of `task` that ended and are not yet settled: removable runtime,
/// kept sessions. A settled container never costs another filesystem call.
fn ended(task: &Task) -> Vec<Recipient> {
    let terminal = matches!(task.status, Status::Done | Status::Dropped);
    let mut out = Vec::new();
    for run in &task.runs {
        let worker = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let run_ended = terminal || container_closure(task, &worker).is_some();
        if run_is_pi_native(run) && run_ended && run.inbox.settled.is_none() {
            out.push(Recipient::Run {
                task: task.id.clone(),
                run: run.id.clone(),
            });
        }
        let unsettled = run.review_runs.iter().filter(|c| c.inbox.settled.is_none());
        for child in unsettled.filter(|c| run_is_pi_native(c)) {
            let reviewer = ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            };
            if run_ended || container_closure(task, &reviewer).is_some() {
                out.push(Recipient::ReviewRun {
                    task: task.id.clone(),
                    run: run.id.clone(),
                    review: child.id.clone(),
                });
            }
        }
    }
    out
}

/// Removes the runtime directory of every ended pi-native container; returns what it removed.
pub fn run_end(ctx: &Ctx, task: &Task) -> Result<Vec<PathBuf>> {
    run_end_in(ctx, task, &super::super::launch::runtime_root())
}

/// `run_end` under an explicit runtime root.
pub fn run_end_in(ctx: &Ctx, task: &Task, root: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for recipient in ended(task) {
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
