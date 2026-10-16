use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use corral_core::{Status, Task};
use serde_json::json;

use crate::Ctx;

/// Creates a task. `project`: explicit key, else inferred from `cwd` via `infer_project`, else None.
/// Unknown explicit project keys are an error listing the known keys.
pub fn new_task(
    ctx: &Ctx,
    title: &str,
    project: Option<&str>,
    cwd: &Path,
    status: Status,
    body: &str,
) -> Result<Task> {
    let origin = if status == Status::Idea {
        corral_core::Origin::Owner
    } else {
        corral_core::Origin::Agent
    };
    new_task_with_origin(ctx, title, project, cwd, status, body, origin)
}

pub fn new_task_with_origin(
    ctx: &Ctx,
    title: &str,
    project: Option<&str>,
    cwd: &Path,
    status: Status,
    body: &str,
    origin: corral_core::Origin,
) -> Result<Task> {
    new_task_initialized(ctx, title, project, cwd, status, body, |task| {
        task.origin = Some(origin);
        Ok(())
    })
}

/// Resolves the project and initializes complete filing metadata before publication.
///
/// # Errors
/// Returns an error for an unknown project or failed task initialization or creation.
pub fn new_task_initialized(
    ctx: &Ctx,
    title: &str,
    project: Option<&str>,
    cwd: &Path,
    status: Status,
    body: &str,
    initialize: impl FnOnce(&mut Task) -> Result<()>,
) -> Result<Task> {
    let project = match project {
        Some(key) => {
            if !ctx.store.config.projects.contains_key(key) {
                let known: Vec<&str> = ctx
                    .store
                    .config
                    .projects
                    .keys()
                    .map(String::as_str)
                    .collect();
                bail!(
                    "unknown project {key:?} (known: {})",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                );
            }
            Some(key.to_string())
        }
        None => infer_project(ctx, cwd),
    };
    ctx.store
        .create_initialized(title, project.as_deref(), status, body, initialize)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The project whose path contains `cwd` (longest match wins); also matches a run worktree
/// `<project>/.worktrees/<id>`. None when outside every project.
pub fn infer_project(ctx: &Ctx, cwd: &Path) -> Option<String> {
    let cwd = canonical(cwd);
    let mut best: Option<(usize, String)> = None;
    for key in ctx.store.config.projects.keys() {
        let Ok(path) = ctx.store.project_path(key) else {
            continue;
        };
        let path = canonical(&path);
        if cwd.starts_with(&path) {
            let depth = path.components().count();
            if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                best = Some((depth, key.clone()));
            }
        }
    }
    best.map(|(_, k)| k)
}

fn manual_status(s: Status) -> bool {
    matches!(s, Status::Idea | Status::Backlog | Status::Ready)
}

/// Manual moves. Allowed targets: Idea, Backlog, Ready, Dropped from Idea/Backlog/Ready; Dropped from anywhere.
/// Anything else errors with "only dispatch, herdr state and `corral done` move tasks into Running, Needs me
/// and Review; approve moves Review to Done" unless `force`. Moving to Ready clears nothing; logs "moved".
pub fn move_task(ctx: &Ctx, id: &str, to: Status, force: bool) -> Result<Task> {
    move_task_with_decision(ctx, id, to, force, None, "owner", false)
}

/// Manual move with its decision author and reason, independent of filing origin.
pub fn move_task_with_decision(
    ctx: &Ctx,
    id: &str,
    to: Status,
    force: bool,
    reason: Option<&str>,
    actor: &str,
    owner_ok: bool,
) -> Result<Task> {
    let mut from = to;
    let task = ctx.store.update(id, |t| {
        if t.launch.is_some() {
            bail!("{} has an unfinished dispatch; dequeue it before moving", t.id);
        }
        from = t.status;
        let allowed = to == Status::Dropped || (manual_status(to) && manual_status(t.status));
        if !allowed && !force {
            bail!(
                "cannot move {} from {} to {}: only dispatch, herdr state and `corral done` move tasks into Running, Needs me and Review; approve moves Review to Done",
                t.id,
                t.status.label(),
                to.label()
            );
        }
        if force
            && to == Status::Review
            && let Some(run) = t.current_run_mut()
            && let Some(review) = run.review.as_ref().filter(|r| r.verdict.is_none())
            && review.sha != corral_git::rev_sha(&run.worktree, "HEAD")?
        {
            crate::reviewer::supersede_review(ctx, run, jiff::Timestamp::now());
            run.autopilot = None;
        }
        let action = if from != to && to == Status::Dropped {
            Some("drop")
        } else if matches!(to, Status::Idea | Status::Backlog)
            && matches!(from, Status::Ready | Status::Running | Status::NeedsMe | Status::Review)
        {
            Some("defer")
        } else {
            None
        };
        if owner_ok && t.effective_origin() == corral_core::Origin::Owner
            && let Some(action) = action
        {
            let reason = reason.unwrap_or("Owner approved manual move");
            if reason.trim().is_empty() {
                bail!("an owner decision requires a reason");
            }
            // Validation is complete. Bind consent to the fresh pre-move revision
            // under this update's lock; the protected save consumes it immediately.
            ctx.store.log_event(
                Some(&t.id),
                "owner_decision",
                json!({"action": action, "reason": reason, "actor": "owner",
                    "source": "flag", "revision": corral_core::owner_revision(t)?}),
            )?;
        }
        t.status = to;
        if to == Status::Ready {
            t.queued = true;
            t.dispatch_hold = false;
        } else {
            t.queued = false;
            t.dispatch_plan = None;
            t.dispatch_hold = to == Status::Backlog && from != Status::Idea;
        }
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "moved",
        json!({ "from": from.as_str(), "to": to.as_str(), "force": force,
            "task_updated": task.updated, "reason": reason, "actor": actor }),
    )?;
    Ok(task)
}

/// (used, limit): used = tasks whose status holds a slot (Running, NeedsMe); limit = config.limits.running.
pub fn slots(ctx: &Ctx) -> Result<(usize, usize)> {
    let used = crate::slot_usage(ctx)?.total();
    Ok((used, ctx.store.config.limits.running))
}
