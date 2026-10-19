//! Durable post-land operations. Callers hold the repository landing lock throughout execution.
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::{ArtifactKind, PostLandHook, Run, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::Ctx;
use crate::hook_command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookAttempt {
    pub number: u32,
    pub started: Timestamp,
    pub finished: Option<Timestamp>,
    pub token: String,
    pub pid: Option<u32>,
    pub process_start: Option<String>,
    pub cwd: PathBuf,
    pub env: std::collections::BTreeMap<String, String>,
    pub log: PathBuf,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
    pub backup: Option<PathBuf>,
    pub installed_sha256: Option<String>,
    pub identity_stdout: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookReceipt {
    pub hook: PostLandHook,
    pub state: HookState,
    pub attempts: Vec<HookAttempt>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostLandOperation {
    pub version: u32,
    pub key: String,
    pub project: String,
    pub repo: PathBuf,
    pub task: String,
    pub run: String,
    pub base: String,
    pub base_before: String,
    pub landed_sha: String,
    pub policy_digest: String,
    pub created: Timestamp,
    /// Written before Git mutation, reconciled against Git if the process stops at that boundary.
    pub landed: bool,
    pub abandoned: bool,
    /// Notification intent is persisted first: at most one delivery attempt per incident.
    pub notified: bool,
    pub hooks: Vec<HookReceipt>,
}

impl PostLandOperation {
    pub fn ready(&self) -> bool {
        self.landed && self.hooks.iter().all(|h| h.state == HookState::Succeeded)
    }

    pub fn blocker(&self) -> Option<String> {
        if self.abandoned || self.ready() {
            return None;
        }
        let receipt = self
            .hooks
            .iter()
            .find(|h| h.state != HookState::Succeeded)?;
        Some(format!(
            "operational readiness {:?}: {} at {} hook {}: {}",
            receipt.state,
            self.project,
            self.landed_sha,
            receipt.hook.name,
            receipt.reason.as_deref().unwrap_or("not completed")
        ))
    }
}

fn directory(ctx: &Ctx) -> PathBuf {
    ctx.store.state_dir().join("post-land")
}

fn path(ctx: &Ctx, op: &PostLandOperation) -> PathBuf {
    directory(ctx).join(format!("{}.json", op.key))
}

pub(crate) fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

pub(crate) fn save(ctx: &Ctx, op: &PostLandOperation) -> Result<()> {
    let dir = directory(ctx);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = path(ctx, op);
    let tmp = path.with_extension("json.tmp");
    let mut file = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    file.write_all(&serde_json::to_vec_pretty(op)?)
        .context("writing post-land receipt")?;
    file.sync_all().context("syncing post-land receipt")?;
    fs::rename(&tmp, &path).context("committing post-land receipt")?;
    #[cfg(unix)]
    File::open(&dir)?
        .sync_all()
        .context("syncing receipt directory")?;
    // Windows cannot FlushFileBuffers on a directory handle. The receipt file
    // above is flushed before its same-volume atomic replacement.
    Ok(())
}

/// Inspect without executing commands or changing receipts.
pub fn post_land_operations(ctx: &Ctx, project: Option<&str>) -> Result<Vec<PostLandOperation>> {
    let dir = directory(ctx);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut operations = Vec::new();
    for entry in entries {
        let file = entry.context("reading receipt directory entry")?.path();
        if file.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let op: PostLandOperation = serde_json::from_slice(
            &fs::read(&file).with_context(|| format!("reading {}", file.display()))?,
        )
        .with_context(|| format!("parsing {}", file.display()))?;
        if project.is_none_or(|p| op.project == p) {
            operations.push(op);
        }
    }
    operations.sort_by(|a, b| a.created.cmp(&b.created).then(a.key.cmp(&b.key)));
    Ok(operations)
}

pub fn operational_blocker(ctx: &Ctx, project: &str) -> Result<Option<String>> {
    Ok(post_land_operations(ctx, Some(project))?
        .iter()
        .find_map(PostLandOperation::blocker))
}

/// Prepare before base mutation so a crash cannot lose the obligation to run hooks.
pub(crate) fn prepare(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    repo: &Path,
    base_before: &str,
    sha: &str,
) -> Result<Option<PostLandOperation>> {
    let Some(project) = task.project.as_deref() else {
        return Ok(None);
    };
    let hooks = &ctx.store.config.projects[project].post_land;
    if hooks.is_empty() || sha == base_before {
        return Ok(None);
    }
    let policy_digest = digest(&(1, "no-coalescing", hooks))?;
    let key = digest(&(project, repo, sha, &policy_digest))?;
    let op = PostLandOperation {
        version: 1,
        key,
        project: project.into(),
        repo: repo.into(),
        task: task.id.clone(),
        run: run.id.clone(),
        base: run.base.clone(),
        base_before: base_before.into(),
        landed_sha: sha.into(),
        policy_digest,
        created: Timestamp::now(),
        landed: false,
        abandoned: false,
        notified: false,
        hooks: hooks
            .iter()
            .map(|hook| HookReceipt {
                hook: hook.clone(),
                state: HookState::Pending,
                attempts: Vec::new(),
                reason: None,
            })
            .collect(),
    };
    // A repeated request must never overwrite successful or uncertain receipts.
    if path(ctx, &op).exists() {
        let previous = serde_json::from_slice(&fs::read(path(ctx, &op))?)
            .context("reading existing operation")?;
        return Ok(Some(previous));
    }
    save(ctx, &op)?;
    Ok(Some(op))
}

fn observe_landing(ctx: &Ctx, op: &mut PostLandOperation) -> Result<()> {
    if op.landed || op.abandoned {
        return Ok(());
    }
    let base = corral_git::rev_sha(&op.repo, &op.base)?;
    if op.base_before != op.landed_sha && corral_git::is_ancestor(&op.repo, &op.landed_sha, &base)?
    {
        op.landed = true;
        ctx.store.update(&op.task, |task| {
            if task.awaiting_owner() && task.current_run().is_some_and(|run| run.id == op.run) {
                task.status = corral_core::Status::Done;
                task.queued = false;
            }
            Ok(())
        })?;
    } else {
        // Failed/abandoned Git mutations never run hooks.
        op.abandoned = true;
    }
    save(ctx, op)
}

/// Reconcile and finish pending hooks, under the same lock used by every corral landing.
pub(crate) fn reconcile_locked(ctx: &Ctx, repo: &Path) -> Result<()> {
    for mut op in post_land_operations(ctx, None)?
        .into_iter()
        .filter(|op| op.repo == repo && !op.abandoned && !op.ready())
    {
        observe_landing(ctx, &mut op)?;
        if op.landed {
            run_locked(ctx, &mut op)?;
        }
    }
    Ok(())
}

pub(crate) fn complete_landing(ctx: &Ctx, op: &mut PostLandOperation) -> Result<()> {
    op.landed = true;
    save(ctx, op)?;
    run_locked(ctx, op)
}

fn same_checkout(op: &PostLandOperation) -> Result<bool> {
    Ok(corral_git::current_branch(&op.repo)? == op.base
        && corral_git::rev_sha(&op.repo, "HEAD")? == op.landed_sha
        && !crate::review::has_tracked_changes(&op.repo)?)
}

fn reconcile_running(ctx: &Ctx, op: &mut PostLandOperation, i: usize) -> Result<()> {
    if !matches!(op.hooks[i].state, HookState::Running | HookState::Uncertain) {
        return Ok(());
    }
    if op.hooks[i].attempts.is_empty() {
        return Ok(());
    }
    if op.hooks[i].state == HookState::Uncertain
        && op.hooks[i]
            .attempts
            .last()
            .is_some_and(|a| a.finished.is_some())
    {
        return Ok(());
    }
    let attempt = op.hooks[i]
        .attempts
        .last()
        .context("running hook has no attempt")?;
    if hook_command::still_running(attempt)? {
        op.hooks[i].state = HookState::Uncertain;
        op.hooks[i].reason = Some("interrupted supervisor; hook process is still running".into());
    } else if op.hooks[i].hook.identity.is_some() && hook_command::observe_identity(op, i).is_ok() {
        op.hooks[i].state = HookState::Succeeded;
        op.hooks[i].reason =
            Some("reconciled from installed executable identity; exit unknown".into());
        if let Some(attempt) = op.hooks[i].attempts.last_mut() {
            attempt.finished = Some(Timestamp::now());
        }
    } else {
        op.hooks[i].state = HookState::Uncertain;
        op.hooks[i].reason = Some(
            "interrupted side effects cannot be proved; explicit acknowledged retry required"
                .into(),
        );
    }
    save(ctx, op)
}

fn run_locked(ctx: &Ctx, op: &mut PostLandOperation) -> Result<()> {
    for i in 0..op.hooks.len() {
        reconcile_running(ctx, op, i)?;
        match op.hooks[i].state {
            HookState::Succeeded => continue,
            HookState::Failed | HookState::Uncertain => break,
            HookState::Running => bail!("unreconciled running hook"),
            HookState::Pending => {}
        }
        if !same_checkout(op)? {
            op.hooks[i].state = HookState::Uncertain;
            op.hooks[i].reason = Some(
                "base checkout no longer matches landed SHA or has tracked changes; no command run"
                    .into(),
            );
            save(ctx, op)?;
            break;
        }
        if let Err(error) = hook_command::execute(ctx, op, i) {
            let interrupted = op.hooks[i].state == HookState::Running
                && op.hooks[i]
                    .attempts
                    .last()
                    .is_some_and(|a| a.finished.is_none());
            op.hooks[i].state = if interrupted {
                HookState::Uncertain
            } else {
                HookState::Failed
            };
            op.hooks[i].reason = Some(format!("{error:#}"));
            if let Some(attempt) = op.hooks[i].attempts.last_mut() {
                if !interrupted {
                    attempt.finished = Some(Timestamp::now());
                }
                attempt.error = Some(format!("{error:#}"));
            }
            if !interrupted && let Err(restore) = hook_command::restore(op, i) {
                op.hooks[i].reason = Some(format!(
                    "{error:#}; prior binary restore failed: {restore:#}"
                ));
            }
            save(ctx, op)?;
            break;
        }
        // A hook must not make a changed checkout look SHA-bound.
        if !same_checkout(op)? {
            op.hooks[i].state = HookState::Uncertain;
            op.hooks[i].reason = Some("checkout changed during hook execution".into());
            save(ctx, op)?;
            break;
        }
    }
    publish(ctx, op)
}

fn publish(ctx: &Ctx, op: &mut PostLandOperation) -> Result<()> {
    if let Some(reason) = op.blocker()
        && !op.notified
    {
        op.notified = true;
        save(ctx, op)?;
        ctx.store.log_event(
            Some(&op.task),
            "blocked",
            json!({"run":op.run, "operation":op.key, "landed":true, "sha":op.landed_sha, "reason":reason}),
        )?;
        if let Err(error) = ctx
            .herdr
            .notify("Post-land operation blocked", &reason, true)
        {
            crate::messages::report(
                Some(&op.task),
                "post-land",
                format!("notification failed: {error:#}"),
            );
        }
    }
    let bytes = serde_json::to_vec_pretty(op)?;
    let receipt_digest = digest(op)?;
    let file = directory(ctx).join(format!("{}-operations.md", op.key));
    let report = format!(
        "# Post-land operations: {}\n\nLanded: {}\nLanded SHA: `{}`\nOperational readiness: {}\nPolicy digest: `{}`\nReceipt digest: `{}`\nCoalescing: none\n\n```json\n{}\n```\n",
        op.project,
        op.landed,
        op.landed_sha,
        if op.ready() {
            "succeeded".to_string()
        } else {
            op.blocker().unwrap_or_else(|| "pending".into())
        },
        op.policy_digest,
        receipt_digest,
        String::from_utf8_lossy(&bytes)
    );
    fs::write(&file, report).context("writing operations artifact")?;
    crate::artifacts::add_artifact(
        ctx,
        &op.task,
        &file,
        Some(ArtifactKind::Evidence),
        Some("Post-land operations"),
    )?;
    for receipt in &op.hooks {
        for attempt in &receipt.attempts {
            if attempt.log.is_file() {
                crate::artifacts::add_artifact(
                    ctx,
                    &op.task,
                    &attempt.log,
                    Some(ArtifactKind::Log),
                    Some(&format!(
                        "Post-land {} attempt {}",
                        receipt.hook.name, attempt.number
                    )),
                )?;
            }
        }
    }
    Ok(())
}

/// Restart reconciliation runs even when the terminal engine is unavailable.
pub fn reconcile_post_land(ctx: &Ctx) -> Result<()> {
    crate::plan_target::reconcile_all(ctx)?;
    let mut repos = std::collections::BTreeSet::new();
    for op in post_land_operations(ctx, None)? {
        if !op.abandoned && !op.ready() {
            repos.insert(op.repo);
        }
    }
    for repo in repos {
        if let Some(_lock) = corral_git::try_landing_lock(&repo)? {
            reconcile_locked(ctx, &repo)?;
        }
    }
    Ok(())
}

/// Resume only the first unsuccessful hook; previous successes never rerun.
pub fn retry_post_land(
    ctx: &Ctx,
    project: &str,
    sha: &str,
    acknowledge_uncertain: bool,
) -> Result<PostLandOperation> {
    let repo = ctx.store.project_path(project)?;
    let _lock = corral_git::landing_lock(&repo)?;
    reconcile_locked(ctx, &repo)?;
    let matches: Vec<_> = post_land_operations(ctx, Some(project))?
        .into_iter()
        .filter(|op| op.landed_sha == sha && !op.abandoned && !op.ready())
        .collect();
    if matches.len() != 1 {
        bail!(
            "expected one incomplete operation for project {project} at exact SHA {sha}; found {}",
            matches.len()
        );
    }
    let mut op = matches
        .into_iter()
        .next()
        .context("operation disappeared")?;
    let receipt = op
        .hooks
        .iter_mut()
        .find(|r| r.state != HookState::Succeeded)
        .context("operation already succeeded")?;
    if receipt.state == HookState::Uncertain && !acknowledge_uncertain {
        bail!("uncertain side effects require --acknowledge-uncertain");
    }
    if receipt
        .attempts
        .last()
        .map(hook_command::still_running)
        .transpose()?
        .unwrap_or(false)
    {
        bail!("hook process is still running; refusing duplicate side effects");
    }
    if receipt.attempts.len() >= usize::try_from(receipt.hook.max_attempts)? {
        bail!(
            "bounded retry exhausted ({} attempts)",
            receipt.hook.max_attempts
        );
    }
    receipt.state = HookState::Pending;
    receipt.reason = None;
    op.notified = false;
    save(ctx, &op)?;
    run_locked(ctx, &mut op)?;
    Ok(op)
}
