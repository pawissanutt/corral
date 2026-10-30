//! Assistant scratch worktrees (T-544 scratch-worktrees). A session creates them from its shell
//! at `<repo>/.worktrees/A-<n>-<slug>` on branch `corral/A-<n>/<slug>`; a spike never lands.
//! Every removal (the session's own discard or the owner's GC) first saves commits beyond the
//! recorded base and uncommitted tracked and untracked changes as a `diff` artifact on the
//! session, and a failed save aborts the removal.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use corral_core::assistant::{AssistantScratch, validate_id};
use corral_core::{ArtifactTier, Origin, Status, Task};
use serde::Serialize;

use crate::Ctx;
use crate::cleanup_worktree::Callers;

mod finish;
mod gc;
mod git;
mod measure;

pub use finish::{Finish, Finished, LABEL, finish, session_for as finish_session};
pub(crate) use finish::{record_landing, release_discarded};
pub use gc::{GcItem, GcReport, gc, orphans};
pub use measure::{Measured, measure};

pub const SPIKE: &str = "spike";
/// Set by an owner-instructed finish; such a scratch belongs to its task from then on.
pub const COWORK: &str = "cowork";

struct Limits {
    max: usize,
    idle_days: i64,
}

fn limits(ctx: &Ctx) -> Limits {
    let config = &ctx.store.config.assistant;
    Limits {
        max: config.scratch_max as usize,
        idle_days: i64::from(config.scratch_idle_days),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScratchRow {
    pub session: String,
    pub session_ended: bool,
    #[serde(flatten)]
    pub scratch: AssistantScratch,
}

#[derive(Debug, Clone, Serialize)]
pub struct Removed {
    pub session: String,
    pub slug: String,
    pub head: Option<String>,
    pub commits: usize,
    pub uncommitted: bool,
    /// Session artifacts holding the saved work; empty when nothing went beyond the base.
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Promoted {
    pub session: String,
    pub slug: String,
    pub task: String,
    pub artifact: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScratchStorage {
    pub session: String,
    pub slug: String,
    pub project: String,
    pub path: PathBuf,
    /// The recorded worktree directory no longer exists.
    pub missing: bool,
    #[serde(flatten)]
    pub measured: Measured,
}

/// The session a scratch command acts for. An assistant shell (`CORRAL_ASSISTANT`) acts only
/// for itself: naming another session refuses `not_own_scratch`. The owner names one.
pub fn session_for(named: Option<&str>) -> Result<String> {
    let caller = std::env::var_os("CORRAL_ASSISTANT")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow!("CORRAL_ASSISTANT is not valid UTF-8"))
        })
        .transpose()?;
    let session = match (caller, named) {
        (Some(own), None) => own,
        (Some(own), Some(named)) if own == named => own,
        (Some(own), Some(named)) => bail!(
            "not_own_scratch: {own} manages only its own scratch worktrees, not {named}'s; nothing changed"
        ),
        (None, Some(named)) => named.to_owned(),
        (None, None) => bail!("--session A-<n> is required outside an assistant shell"),
    };
    validate_id(&session)?;
    Ok(session)
}

pub(crate) fn validate_slug(slug: &str) -> Result<()> {
    let valid = (1..=40).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--");
    if !valid {
        bail!("scratch slug must be 1-40 lowercase letters, digits and single hyphens: {slug:?}");
    }
    Ok(())
}

fn disk_check(ctx: &Ctx) -> Result<()> {
    let snapshot = ctx.resource_probe.sample(&ctx.store)?;
    let min = ctx.store.config.resources.min_disk_bytes()?;
    if let Some(disk) = snapshot.disks.iter().find(|disk| disk.free_bytes < min) {
        bail!(
            "disk_low: {} has {} bytes free, below resources.min_disk_free ({min} bytes); no scratch was created",
            disk.path.display(),
            disk.free_bytes
        );
    }
    Ok(())
}

/// Creates a spike scratch for an open session, from `base` or the project's base branch.
pub fn create(
    ctx: &Ctx,
    session: &str,
    project: &str,
    slug: Option<&str>,
    base: Option<&str>,
) -> Result<AssistantScratch> {
    // Held from the workspace count through recording, so concurrent creators respect the cap.
    let _creating = lock(ctx, "scratch-create")?;
    let current = ctx.store.get_assistant(session)?;
    if current.ended.is_some() {
        bail!("assistant_ended: {session} has ended; scratch worktrees belong to open sessions");
    }
    let total: usize = ctx
        .store
        .list_assistants()?
        .iter()
        .map(|s| s.scratch.len())
        .sum();
    let max = limits(ctx).max;
    if total >= max {
        bail!(
            "scratch_cap: {total} scratch worktrees exist (assistant.scratch_max {max}); discard one, or the owner runs `corral assistant scratch gc`"
        );
    }
    disk_check(ctx)?;
    let repo = ctx
        .store
        .project_path(project)?
        .canonicalize()
        .with_context(|| format!("resolving project {project:?}"))?;
    let free = |slug: &str| -> Result<bool> {
        Ok(!current.scratch.iter().any(|s| s.slug == slug)
            && !repo
                .join(".worktrees")
                .join(format!("{session}-{slug}"))
                .exists()
            && !git::branch_exists(&repo, &format!("corral/{session}/{slug}"))?)
    };
    let slug = match slug {
        Some(slug) => {
            validate_slug(slug)?;
            if !free(slug)? {
                bail!(
                    "scratch_exists: {session} already has scratch {slug}, or its worktree or branch exists"
                );
            }
            slug.to_owned()
        }
        None => {
            let mut n = 1;
            while !free(&format!("scratch-{n}"))? {
                n += 1;
            }
            format!("scratch-{n}")
        }
    };
    let branch = format!("corral/{session}/{slug}");
    corral_git::validate_branch_name(&branch)?;
    let base = match base.map(str::to_owned) {
        Some(base) => base,
        None => match ctx
            .store
            .config
            .projects
            .get(project)
            .and_then(|p| p.base.clone())
        {
            Some(base) => base,
            None => corral_git::current_branch(&repo)?,
        },
    };
    let base_sha = corral_git::rev_sha(&repo, &format!("{base}^{{commit}}"))
        .with_context(|| format!("resolving scratch base {base:?}"))?;
    let path = repo.join(".worktrees").join(format!("{session}-{slug}"));
    corral_git::worktree_add(&repo, &path, &branch, &base_sha)?;
    let scratch = AssistantScratch {
        slug,
        project: project.to_owned(),
        path,
        branch,
        base: base_sha,
        created: corral_core::clock::now(),
        kind: SPIKE.into(),
        bytes: None,
        measured_at: None,
    };
    let recorded = ctx.store.update_assistant(session, |s| {
        if s.ended.is_some() || s.scratch.iter().any(|x| x.slug == scratch.slug) {
            bail!(
                "scratch_exists: {session} ended or recorded {} concurrently",
                scratch.slug
            );
        }
        s.scratch.push(scratch.clone());
        Ok(())
    });
    if let Err(error) = recorded {
        // The new worktree holds nothing yet, so removing it loses no work.
        if let Err(cleanup) = crate::cleanup_worktree::remove_scratch(
            &repo,
            &scratch.path,
            &scratch.branch,
            Callers::Exempt,
        ) {
            return Err(error.context(format!(
                "removing the unrecorded scratch also failed: {cleanup:#}"
            )));
        }
        return Err(error);
    }
    ctx.store.log_event(
        None,
        "assistant_scratch_created",
        serde_json::json!({
            "session": session, "slug": scratch.slug, "project": project,
            "path": scratch.path, "branch": scratch.branch, "base": scratch.base,
        }),
    )?;
    Ok(scratch)
}

pub fn list(ctx: &Ctx, session: Option<&str>) -> Result<Vec<ScratchRow>> {
    if let Some(session) = session {
        ctx.store.get_assistant(session)?;
    }
    Ok(ctx
        .store
        .list_assistants()?
        .into_iter()
        .filter(|s| session.is_none_or(|id| s.id == id))
        .flat_map(|s| {
            let ended = s.ended.is_some();
            let id = s.id;
            s.scratch.into_iter().map(move |scratch| ScratchRow {
                session: id.clone(),
                session_ended: ended,
                scratch,
            })
        })
        .collect())
}

/// The recorded scratch, refusing a record that names a worktree or branch not its own.
fn find(ctx: &Ctx, session: &str, slug: &str) -> Result<AssistantScratch> {
    validate_slug(slug)?;
    let scratch = ctx
        .store
        .get_assistant(session)?
        .scratch
        .into_iter()
        .find(|s| s.slug == slug)
        .ok_or_else(|| anyhow!("unknown_scratch: {session} has no scratch {slug:?}"))?;
    let name = format!("{session}-{slug}");
    if scratch.path.file_name() != Some(name.as_ref())
        || scratch.branch != format!("corral/{session}/{slug}")
    {
        bail!(
            "scratch record {session}/{slug} names {} on {}, not its own worktree; inspect it by hand",
            scratch.path.display(),
            scratch.branch
        );
    }
    Ok(scratch)
}

/// The repository whose `.worktrees` holds the scratch, independent of later config edits.
fn repo_of(scratch: &AssistantScratch) -> Result<PathBuf> {
    scratch
        .path
        .parent()
        .filter(|dir| dir.file_name().is_some_and(|name| name == ".worktrees"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("{} is not under .worktrees", scratch.path.display()))
}

/// A task run or co-work finish that owns the scratch's lifecycle now.
fn holder(tasks: &[Task], scratch: &AssistantScratch) -> Option<String> {
    if scratch.kind == COWORK {
        return Some("a co-work finish".into());
    }
    tasks.iter().find_map(|task| {
        task.runs
            .iter()
            .find(|run| run.worktree == scratch.path)
            .map(|run| format!("{} {}", task.id, run.id))
    })
}

/// A cross-process lock under `.corral/lock/`: scratch creation, or removal of one scratch.
fn lock(ctx: &Ctx, name: &str) -> Result<std::fs::File> {
    let dir = ctx.store.dir().join("lock");
    std::fs::create_dir_all(&dir).context("creating lock directory")?;
    let path = dir.join(format!("{name}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock().context("locking scratch")?;
    Ok(file)
}

fn preserve(
    ctx: &Ctx,
    session: &str,
    scratch: &AssistantScratch,
    repo: &Path,
    work: &git::Work,
) -> Result<Vec<String>> {
    let saved = git::preservation(session, repo, scratch, work)?;
    if saved.is_empty() {
        bail!(
            "{session}/{} has work git could not export; nothing was removed",
            scratch.slug
        );
    }
    let temp = tempfile::tempdir().context("creating scratch patch directory")?;
    let mut paths = Vec::new();
    for item in saved {
        let file = temp.path().join(&item.name);
        std::fs::write(&file, &item.bytes).context("writing scratch patch")?;
        let artifact =
            crate::add_session_artifact(ctx, session, &file, Some(item.kind), Some(&item.title))
                .context("saving the scratch's work failed; nothing was removed")?;
        // A later diff on the session must not demote this one to transient retention.
        crate::assistant_artifacts::update(ctx, session, &artifact.path, |artifact| {
            artifact.tier_override = Some(ArtifactTier::Keep);
        })
        .context("protecting the saved scratch work failed; nothing was removed")?;
        paths.push(artifact.path);
    }
    Ok(paths)
}

/// Preserve, remove the worktree and branch, then forget the record (discard and GC).
fn remove(
    ctx: &Ctx,
    session: &str,
    scratch: &AssistantScratch,
    callers: Callers,
) -> Result<Removed> {
    let repo = repo_of(scratch)?;
    let busy = crate::cleanup_worktree::scratch_residents(&scratch.path, callers)?;
    if !busy.is_empty() {
        bail!(
            "scratch_busy: processes {busy:?} use {}; leave the directory and retry; nothing was removed",
            scratch.path.display()
        );
    }
    let work = git::work(&repo, scratch)?;
    let artifacts = if work.any() {
        preserve(ctx, session, scratch, &repo, &work)?
    } else {
        Vec::new()
    };
    crate::cleanup_worktree::remove_scratch(&repo, &scratch.path, &scratch.branch, callers)?;
    ctx.store.update_assistant(session, |s| {
        s.scratch.retain(|x| x.slug != scratch.slug);
        Ok(())
    })?;
    Ok(Removed {
        session: session.to_owned(),
        slug: scratch.slug.clone(),
        head: work.head,
        commits: work.commits.len(),
        uncommitted: work.dirty,
        artifacts,
    })
}

/// The session's own disposal: no confirmation, work saved first. Its own shell and this
/// command may run from inside the scratch.
pub fn discard(ctx: &Ctx, session: &str, slug: &str) -> Result<Removed> {
    validate_id(session)?;
    validate_slug(slug)?;
    let _guard = lock(ctx, &format!("scratch-{session}-{slug}"))?;
    let scratch = find(ctx, session, slug)?;
    if let Some(holder) = holder(&ctx.store.list()?, &scratch) {
        bail!(
            "scratch_attached: {session}/{slug} belongs to {holder}; it is removed with that task's run"
        );
    }
    let removed = remove(ctx, session, &scratch, Callers::Exempt)?;
    ctx.store.log_event(
        None,
        "assistant_scratch_discarded",
        serde_json::json!({
            "session": session, "slug": slug, "project": scratch.project,
            "branch": scratch.branch, "head": removed.head, "artifacts": removed.artifacts,
        }),
    )?;
    Ok(removed)
}

/// Files a Backlog task through the `corral new` path with the spike diff attached. The
/// scratch is left exactly as it was.
pub fn promote(
    ctx: &Ctx,
    session: &str,
    slug: &str,
    title: &str,
    project: Option<&str>,
    body: &str,
) -> Result<Promoted> {
    let scratch = find(ctx, session, slug)?;
    let repo = repo_of(&scratch)?;
    let work = git::work(&repo, &scratch)?;
    if !work.any() {
        bail!(
            "scratch_empty: {session}/{slug} has no commits or changes beyond its base; nothing was filed"
        );
    }
    let patch = git::net_patch(session, &repo, &scratch, &work)?;
    let max = ctx.store.config.artifacts.max_bytes;
    if patch.len() as u64 > max {
        bail!(
            "the {session}/{slug} diff is {} bytes, larger than artifacts.max_bytes {max}; nothing was filed",
            patch.len()
        );
    }
    let temp = tempfile::tempdir().context("creating scratch patch directory")?;
    let file = temp.path().join(format!("{session}-{slug}.diff"));
    std::fs::write(&file, patch).context("writing scratch patch")?;
    let mut text = body.trim_end().to_owned();
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(&format!(
        "Promoted from assistant scratch {session}/{slug} (branch `{}`, base `{}`, head `{}`). The spike diff is attached as an artifact; the scratch itself is unchanged and never lands.\n",
        scratch.branch,
        scratch.base,
        work.head.as_deref().unwrap_or("none")
    ));
    let project = project.unwrap_or(&scratch.project);
    let task = crate::new_task_initialized(
        ctx,
        title,
        Some(project),
        ctx.store.root(),
        Status::Backlog,
        &text,
        |task| {
            // The `corral new` default filer; the event log carries the assistant attribution.
            task.origin = Some(Origin::Owner);
            Ok(())
        },
    )?;
    let artifact = crate::artifacts::add_promotion_artifact(
        ctx,
        &task.id,
        &file,
        &format!("Spike diff from scratch {session}/{slug}"),
    )
    .with_context(|| format!("filed {} but attaching the scratch diff failed", task.id))?;
    ctx.store.log_event(
        Some(&task.id),
        "assistant_scratch_promoted",
        serde_json::json!({
            "session": session, "slug": slug, "branch": scratch.branch,
            "head": work.head, "artifact": artifact.path,
        }),
    )?;
    Ok(Promoted {
        session: session.to_owned(),
        slug: slug.to_owned(),
        task: task.id,
        artifact: artifact.path,
    })
}

/// Measures every recorded scratch for `corral stats --storage`, without recording anything.
pub fn storage(ctx: &Ctx) -> Result<Vec<ScratchStorage>> {
    let mut rows = Vec::new();
    for session in ctx.store.list_assistants()? {
        for scratch in session.scratch {
            let missing = !scratch.path.is_dir();
            let measured = if missing {
                Measured::default()
            } else {
                measure(&scratch.path)?
            };
            rows.push(ScratchStorage {
                session: session.id.clone(),
                slug: scratch.slug,
                project: scratch.project,
                path: scratch.path,
                missing,
                measured,
            });
        }
    }
    Ok(rows)
}
