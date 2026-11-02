//! Raw transcript provenance. Ownership is keyed by task, parent run and exact content,
//! independently of an artifact's presentation name or title. Multiple containers can own
//! the same content; the artifact/blob remains deduplicated.
use std::path::Path;

use anyhow::{Context, Result};
use corral_core::agent::ContainerRef;
use corral_core::records::RecordArea;
use corral_core::{Artifact, Run, Status, Task};

use crate::Ctx;

#[cfg(test)]
#[path = "transcript_registration_tests.rs"]
mod registration_tests;
#[cfg(test)]
#[path = "transcripts_tests.rs"]
mod tests;

const OWNERS: RecordArea<Vec<ContainerRef>> = RecordArea::new("transcript-owners");
const CAPTURED: RecordArea<bool> = RecordArea::new("worker-transcripts-captured");

fn key(task: &str, run: &str, sha: &str) -> String {
    format!("{task}/{run}/{sha}")
}

pub(crate) fn owners(ctx: &Ctx, task: &str, artifact: &Artifact) -> Result<Vec<ContainerRef>> {
    Ok(OWNERS
        .load(&ctx.store, &key(task, &artifact.run, &artifact.sha256))?
        .unwrap_or_default())
}

/// Record provenance before attachment, so even an interrupted promotion cannot expose a
/// transcript to generic purge. Retrying attaches the same content and keeps every owner.
pub(crate) fn promote(
    ctx: &Ctx,
    task: &str,
    run: &str,
    owner: ContainerRef,
    path: &Path,
) -> Result<Artifact> {
    promote_with_title(ctx, task, run, owner, path, "Raw agent transcript")
}

pub(crate) fn promote_with_title(
    ctx: &Ctx,
    task: &str,
    run: &str,
    owner: ContainerRef,
    path: &Path,
    title: &str,
) -> Result<Artifact> {
    let owned = match &owner {
        ContainerRef::Worker { task: t, run: r } => t == task && r == run,
        ContainerRef::Reviewer {
            task: t,
            run: r,
            review,
        } => t == task && r == run && ctx.store.head(task)?.review_child(run, review)?.is_some(),
        ContainerRef::Pm
        | ContainerRef::Planner { .. }
        | ContainerRef::PlanReviewer { .. }
        | ContainerRef::Assistant { .. } => false,
    };
    anyhow::ensure!(owned, "transcript owner does not match its task/run");
    let meta = std::fs::symlink_metadata(path).context("inspecting transcript")?;
    anyhow::ensure!(meta.is_file(), "transcript must be a regular file");
    anyhow::ensure!(
        meta.len() <= ctx.store.config.artifacts.max_bytes,
        "transcript exceeds artifact max_bytes"
    );
    let bytes = std::fs::read(path).context("reading transcript for promotion")?;
    let sha = corral_core::agent::paths::sha256_hex(&bytes);
    let _lease = ctx
        .store
        .try_lock("transcript-promotion")?
        .context("another transcript promotion is running")?;
    let key = key(task, run, &sha);
    let mut owners = OWNERS.load(&ctx.store, &key)?.unwrap_or_default();
    if !owners.contains(&owner) {
        owners.push(owner);
        OWNERS.save(&ctx.store, &key, &owners)?;
    }
    // Attach the bytes we classified, not a potentially still-changing provider file.
    crate::artifacts::add_transcript_bytes(ctx, task, run, path, &bytes, title)
}

/// Promote only this container's direct regular session files. Child review directories
/// are separate owners, and symlinks/special files are never followed or read.
pub(crate) fn sessions(ctx: &Ctx, owner: ContainerRef) -> Result<()> {
    let (task, run) = match &owner {
        ContainerRef::Worker { task, run } | ContainerRef::Reviewer { task, run, .. } => {
            (task, run)
        }
        ContainerRef::Pm
        | ContainerRef::Planner { .. }
        | ContainerRef::PlanReviewer { .. }
        | ContainerRef::Assistant { .. } => return Ok(()),
    };
    let dir = corral_core::agent::paths::container_sessions_dir(&ctx.store, &owner);
    let listing = match std::fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("listing retained Pi sessions"),
    };
    for entry in listing {
        let entry = entry.context("listing retained Pi session")?;
        if entry
            .file_type()
            .context("inspecting Pi session")?
            .is_file()
        {
            promote(ctx, task, run, owner.clone(), &entry.path())?;
        }
    }
    Ok(())
}

/// Legacy provider transcripts are copied, never removed from the provider's directory.
pub(crate) fn worker(ctx: &Ctx, task: &str, run: &Run, final_capture: bool) -> Result<()> {
    if crate::agent::hooks::run_is_corral_pi(run) {
        return sessions(
            ctx,
            ContainerRef::Worker {
                task: task.into(),
                run: run.id.clone(),
            },
        );
    }
    if !crate::usage::has_transcript(run, &ctx.store) {
        return Ok(());
    }
    let captured = format!("{task}/{}", run.id);
    if CAPTURED.load(&ctx.store, &captured)?.unwrap_or(false) {
        return Ok(());
    }
    let bytes = crate::usage::transcript_run(run, &ctx.store)?;
    if bytes.is_empty() {
        return Ok(());
    }
    let file = tempfile::Builder::new()
        .suffix(".jsonl")
        .tempfile()
        .context("creating worker transcript")?;
    std::fs::write(file.path(), bytes).context("writing worker transcript")?;
    promote(
        ctx,
        task,
        &run.id,
        ContainerRef::Worker {
            task: task.into(),
            run: run.id.clone(),
        },
        file.path(),
    )?;
    if final_capture {
        CAPTURED.save(&ctx.store, &captured, &true)?;
    }
    Ok(())
}

pub(crate) fn run_end(ctx: &Ctx, task: &Task) -> Result<()> {
    let terminal = matches!(task.status, Status::Done | Status::Dropped);
    for run in &task.runs {
        let owner = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        if (terminal || corral_core::agent::container_closure(task, &owner).is_some())
            && run.inbox.settled.is_none()
        {
            worker(ctx, &task.id, run, true)?;
        }
        for child in &run.review_runs {
            let owner = ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: child.id.clone(),
            };
            if crate::agent::hooks::run_is_corral_pi(child)
                && child.inbox.settled.is_none()
                && (terminal || corral_core::agent::container_closure(task, &owner).is_some())
            {
                sessions(ctx, owner)?;
            }
        }
    }
    Ok(())
}
