//! `--evidence PATH` on `verdict`, `plan verdict` and `plan aggregate-verdict` (plan T-253
//! section 8). The set is checked before anything is recorded: every file must be a regular
//! file reached without symlinks from the attempt's scratch directory or the reviewed worktree,
//! and the set together must fit `review_evidence_max`. A refused set records no verdict. An
//! accepted set is stored as blobs first and attached as artifacts inside the same task update
//! that records the verdict, so the verdict and its evidence commit together or not at all.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use corral_core::{Artifact, ArtifactKind, PlanRunState, Task};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::Ctx;
use crate::artifact_storage::{ARTIFACT_BLOBS, blob_key, record_keys, store_blob};

#[derive(Debug)]
struct File {
    name: String,
    source: PathBuf,
    sha256: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct Evidence {
    files: Vec<File>,
}

/// `path` relative to `root` when it is reached from `root` through real directories only.
fn beneath(path: &Path, real: &Path, root: &Path) -> bool {
    let Ok(canonical_root) = std::fs::canonicalize(root) else {
        return false;
    };
    path.strip_prefix(root)
        .is_ok_and(|rel| canonical_root.join(rel) == real)
}

impl Evidence {
    /// Checks `paths` against the allowed `roots` and the configured budget, reading each file
    /// once; nothing is stored.
    pub(crate) fn check(ctx: &Ctx, paths: &[PathBuf], roots: &[&Path]) -> Result<Self> {
        let budget = ctx.store.config.storage.review_evidence_max_bytes()?;
        let mut files: Vec<File> = Vec::new();
        let mut total = 0u64;
        for given in paths {
            let path = std::path::absolute(given)
                .with_context(|| format!("resolving evidence {}", given.display()))?;
            let meta = std::fs::symlink_metadata(&path)
                .with_context(|| format!("evidence {} not found", given.display()))?;
            ensure!(
                !meta.file_type().is_symlink(),
                "evidence {} is a symlink; attach the file itself",
                given.display()
            );
            ensure!(
                meta.is_file(),
                "evidence {} is not a regular file",
                given.display()
            );
            let real = std::fs::canonicalize(&path)
                .with_context(|| format!("resolving evidence {}", given.display()))?;
            if !roots.iter().any(|root| beneath(&path, &real, root)) {
                bail!(
                    "evidence {} is not inside the attempt directory or the reviewed worktree ({}), or is reached through a symlink",
                    given.display(),
                    roots
                        .iter()
                        .map(|root| root.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            total = total.saturating_add(meta.len());
            ensure!(
                total <= budget,
                "evidence set exceeds review_evidence_max ({}): {total} bytes or more",
                ctx.store.config.storage.review_evidence_max
            );
            let bytes = std::fs::read(&real)
                .with_context(|| format!("reading evidence {}", given.display()))?;
            ensure!(
                u64::try_from(bytes.len())? == meta.len(),
                "evidence {} changed while it was read",
                given.display()
            );
            if files.iter().any(|file| file.source == real) {
                continue;
            }
            let name = real
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .with_context(|| format!("evidence {} has no file name", given.display()))?;
            files.push(File {
                name,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                source: real,
                bytes,
            });
        }
        Ok(Self { files })
    }

    /// Stores every file as a blob of `task`, holding the artifact storage lock until the
    /// returned set is attached or released.
    pub(crate) fn stage(&self, ctx: &Ctx, task: &str) -> Result<Staged<'_>> {
        if self.files.is_empty() {
            return Ok(Staged {
                files: &self.files,
                created: Vec::new(),
                task: task.to_owned(),
                _lock: None,
            });
        }
        let lock = crate::artifact_storage::storage_lock(ctx)?;
        let mut staged = Staged {
            files: &self.files,
            created: Vec::new(),
            task: task.to_owned(),
            _lock: Some(lock),
        };
        for file in &self.files {
            match store_blob(ctx, task, &file.sha256, &file.bytes) {
                Ok(true) => staged.created.push(file.sha256.clone()),
                Ok(false) => {}
                Err(error) => {
                    staged.release(ctx)?;
                    return Err(error);
                }
            }
        }
        Ok(staged)
    }
}

/// Evidence stored as blobs and waiting for the verdict's update.
pub(crate) struct Staged<'a> {
    files: &'a [File],
    created: Vec<String>,
    task: String,
    _lock: Option<crate::artifact_storage::StorageGuard>,
}

/// The run a verdict's evidence attaches to: the open plan run, else the current run, else R0,
/// as `corral artifact add` chooses.
pub(crate) fn current_target(task: &Task) -> String {
    task.plan_runs
        .last()
        .filter(|run| run.state != PlanRunState::Approved)
        .map(|run| run.id.clone())
        .or_else(|| task.current_run().map(|run| run.id.clone()))
        .unwrap_or_else(|| "R0".to_string())
}

/// The roots a plan verdict's evidence may come from: the review attempt's scratch directory
/// and the output directory of the planner run that produced plan `plan`.
pub(crate) fn plan_roots(
    ctx: &Ctx,
    task: &Task,
    plan: &str,
    attempt: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Some(review) = attempt {
        let dir = crate::scratch_reclaim::Attempt::PlanReview {
            task: &task.id,
            plan,
            review,
        };
        roots.push(dir.dir(ctx)?);
    }
    let run = task
        .plans
        .iter()
        .find(|p| p.id == plan)
        .and_then(|p| p.run.as_ref());
    if let Some(run) = run.and_then(|run| task.plan_runs.iter().find(|r| &r.id == run)) {
        roots.push(run.output_dir.clone());
    }
    Ok(roots)
}

impl Staged<'_> {
    /// Inside the verdict's update: attaches every file to `run` with one `artifact` event each.
    pub(crate) fn attach(&self, ctx: &Ctx, task: &mut Task, run: &str) -> Result<()> {
        for file in self.files {
            if task
                .artifacts
                .iter()
                .any(|a| a.run == run && a.sha256 == file.sha256 && a.purged_at.is_none())
            {
                continue;
            }
            let artifact = Artifact {
                run: run.to_owned(),
                kind: ArtifactKind::guess(&file.name),
                title: format!("Evidence: {}", file.name),
                path: format!(
                    "artifacts/{}/{run}/{}-{}",
                    task.id,
                    &file.sha256[..8],
                    file.name
                ),
                source: file.source.to_string_lossy().into_owned(),
                sha256: file.sha256.clone(),
                bytes: u64::try_from(file.bytes.len())?,
                added: corral_core::clock::now(),
                tier_override: None,
                compressed_bytes: None,
                purged_at: None,
            };
            task.artifacts.push(artifact.clone());
            record_keys(ctx, task, task.artifacts.len() - 1, true)?;
            ctx.store.log_event(
                Some(&task.id),
                "artifact",
                json!({ "run": artifact.run, "kind": artifact.kind.as_str(), "title": artifact.title, "path": artifact.path }),
            )?;
        }
        Ok(())
    }

    /// After a verdict that did not commit: drops the blob references this set added.
    pub(crate) fn release(&self, ctx: &Ctx) -> Result<()> {
        let held = ctx.store.get(&self.task)?;
        for sha in &self.created {
            if !held
                .artifacts
                .iter()
                .any(|a| a.sha256 == *sha && a.purged_at.is_none())
            {
                ctx.store
                    .delete_record(ARTIFACT_BLOBS, &blob_key(&self.task, sha))?;
            }
        }
        Ok(())
    }
}
