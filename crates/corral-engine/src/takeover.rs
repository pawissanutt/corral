//! Task checkout ownership shared by engine writers and the assistant host.
//!
//! Lock files are permanent. Removing even an idle one would allow later callers
//! to lock a different inode while a previously opened descriptor still exists.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::{Engine, as_refusal, refuse};
#[cfg(unix)]
use crate::{Run, Runner, Status, Task};

pub(crate) mod identity;

const RESERVED: &str =
    "task checkout is reserved by an assistant session; hand it back before retrying";

/// The host compares the entire snapshot after locking and before writing.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverSnapshot {
    pub task: String,
    pub run: String,
    pub checkout: PathBuf,
    pub lock: PathBuf,
    pub device: String,
    pub inode: String,
    pub lock_device: String,
    pub lock_inode: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriterLock {
    pub lock: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_inode: Option<String>,
}

pub(crate) type CheckoutGuard = corral_git::CommandLease;

pub(crate) fn is_checkout_reserved(error: &anyhow::Error) -> bool {
    as_refusal(error).is_some_and(|r| r.reason == RESERVED)
}

impl Engine {
    pub(crate) fn checkout_writer(&self, id: &str) -> Result<Vec<CheckoutGuard>> {
        self.try_checkout_writer(id)?
            .ok_or_else(|| refuse(RESERVED, vec![]))
    }

    pub(crate) fn try_checkout_writer(&self, id: &str) -> Result<Option<Vec<CheckoutGuard>>> {
        let task = self.store.require_task(id)?;
        let mut repos = Vec::new();
        // A reprojected task may still have an older checkout. Protect both routes;
        // launch also pins the exact checkout chosen inside its transaction.
        if let Some(run) = self.store.latest_run(id, crate::Role::Worker)? {
            let repo = crate::checkout::Checkout::from_run(&run)?.repo;
            if repo
                .try_exists()
                .context("inspect previous task repository")?
            {
                repos.push(repo);
            }
        }
        if let Some(project) = self.project_for(&task)? {
            let repo = self.root.join(project.path);
            if repo
                .try_exists()
                .context("inspect current task repository")?
            {
                repos.push(repo);
            }
        }
        repos.sort_unstable();
        repos.dedup();
        let mut paths = repos
            .iter()
            .map(|repo| repo_lock_path(repo, id))
            .collect::<Result<Vec<_>>>()?;
        if paths.is_empty() {
            paths.push(self.state_dir.join("locks").join(lock_name(id)?));
        }
        paths.sort_unstable();
        paths.dedup();
        let mut guards = Vec::with_capacity(paths.len());
        for path in paths {
            let Some(guard) = try_writer(&path)? else {
                return Ok(None);
            };
            guards.push(guard);
        }
        Ok(Some(guards))
    }

    pub(crate) fn checkout_repo_writer(&self, repo: &Path, id: &str) -> Result<CheckoutGuard> {
        try_writer(&repo_lock_path(repo, id)?)?.ok_or_else(|| refuse(RESERVED, vec![]))
    }

    // Landing onto another task's checked-out branch also writes that checkout.
    // The caller holds the repository landing lock while resolving and using it.
    pub(crate) fn landing_target_writer(
        &self,
        repo: &Path,
        target: &str,
    ) -> Result<Option<CheckoutGuard>> {
        let Some(path) = crate::checkout::branch_worktree(repo, target)? else {
            return Ok(None);
        };
        self.landing_checkout_writer(repo, &path)
    }

    pub(crate) fn landing_checkout_writer(
        &self,
        repo: &Path,
        path: &Path,
    ) -> Result<Option<CheckoutGuard>> {
        let path = path
            .canonicalize()
            .context("resolve landing target checkout")?;
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|id| is_task_id(id))
        else {
            return Ok(None);
        };
        if path
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|name| name != ".worktrees")
        {
            return Ok(None);
        }
        self.checkout_repo_writer(repo, id).map(Some)
    }

    #[cfg(unix)]
    fn takeover_lock_path(&self, id: &str) -> Result<PathBuf> {
        let task = self.store.require_task(id)?;
        let run = self.last_worker(&self.store, &task)?;
        repo_lock_path(&self.require_registered_path(&task, &run)?, id)
    }

    /// Preview an eligible task; the exclusive probe is released on return.
    #[cfg(unix)]
    pub fn takeover_probe(&self, id: &str) -> Result<TakeoverSnapshot> {
        let path = self.takeover_lock_path(id)?;
        let file = open_checkout_lock(&path, true)?;
        if !try_lock(file.try_lock())? {
            return Err(refuse(
                "task checkout already has an engine writer or assistant session",
                vec![],
            ));
        }
        let result = self.takeover_snapshot(id, path, &file);
        // An unrelated concurrent fork can briefly copy a CLOEXEC descriptor.
        // This probe doesn't enroll the lock in any child, so unlock explicitly.
        file.unlock().context("release task checkout probe")?;
        result
    }

    /// Recheck the task with the trusted host already holding its own exclusive
    /// descriptor. A contended shared probe confirms exclusivity, not ownership;
    /// the host must compare the identity against its actual descriptor.
    #[cfg(unix)]
    pub fn takeover_verify(&self, id: &str) -> Result<TakeoverSnapshot> {
        let path = self.takeover_lock_path(id)?;
        let file = open_checkout_lock(&path, false)?;
        if try_lock(file.try_lock_shared())? {
            return Err(refuse(
                "assistant host does not hold an exclusive task checkout lock",
                vec![],
            ));
        }
        self.takeover_snapshot(id, path, &file)
    }

    /// Locate the same lock for a Code/Build host that will hold a shared writer
    /// guard. Unlike takeover, a writer need not wait for an ended worker.
    #[cfg(unix)]
    pub fn takeover_writer_lock(&self, id: &str, checkout: &Path) -> Result<WriterLock> {
        let Some((task, run)) = self.store.read_snapshot(|| {
            self.store
                .get_task(id)?
                .map(|task| {
                    // A checkout may already exist while dispatch's run transaction
                    // is still uncommitted. Do not let a Code host start unguarded.
                    self.last_worker(&self.store, &task).map(|run| (task, run))
                })
                .transpose()
        })?
        else {
            return Ok(WriterLock {
                lock: None,
                lock_device: None,
                lock_inode: None,
            });
        };
        let repo = self.require_registered_path(&task, &run)?;
        ensure!(
            checkout.as_os_str() == run.worktree.as_os_str(),
            "writer checkout does not match the registered task worktree"
        );
        crate::checkout::verify_linked_checkout(&repo, &run.worktree, &run.branch, false)?;
        let lock = repo_lock_path(&repo, id)?;
        let file = open_checkout_lock(&lock, true)?;
        let (device, inode) = file_identity(&require_visible_lock(&lock, &file)?);
        Ok(WriterLock {
            lock: Some(lock),
            lock_device: Some(device),
            lock_inode: Some(inode),
        })
    }

    #[cfg(unix)]
    fn takeover_snapshot(&self, id: &str, lock: PathBuf, file: &File) -> Result<TakeoverSnapshot> {
        self.store.read_snapshot(|| {
            let task = self.store.require_task(id)?;
            if matches!(task.status, Status::Done | Status::Dropped) {
                return Err(refuse(
                    "takeover is unavailable for a terminal task",
                    vec![],
                ));
            }
            if self.store.has_queued_repair(id)? {
                return Err(refuse(
                    "takeover is unavailable while a task repair is queued",
                    vec![],
                ));
            }
            let worker = self.last_worker(&self.store, &task)?;
            if worker.ended.is_none() {
                return Err(refuse(
                    "the current worker run has not ended; takeover does not stop it",
                    vec![],
                ));
            }
            if worker.outcome == Some(crate::Outcome::Landed) {
                return Err(refuse(
                    "the latest worker landed; this is not its retained checkout",
                    vec![],
                ));
            }
            self.require_previous_gate_closed_read_only(id)?;
            self.require_takeover_sessions_closed(id)?;
            let repo = self.require_registered_path(&task, &worker)?;
            ensure!(
                repo_lock_path(&repo, id)? == lock,
                "registered checkout moved to another lock namespace"
            );
            let registered = identity::CheckoutIdentity::load(&self.store, &worker)?.ok_or_else(|| refuse(
                format!("worker {} has no registered checkout identity; start a new worker run before requesting takeover", worker.id),
                vec![],
            ))?;
            let checkout = worker.worktree;
            crate::checkout::verify_linked_checkout(&repo, &checkout, &worker.branch, true)?;
            if identity::CheckoutIdentity::capture(&checkout).as_ref() != Some(&registered) {
                return Err(refuse(format!("the checkout no longer matches the identity registered for worker {}", worker.id), vec![]));
            }
            let (device, inode) = registered.directory_identity();
            let (lock_device, lock_inode) = file_identity(&require_visible_lock(&lock, file)?);
            Ok(TakeoverSnapshot {
                task: id.to_owned(),
                run: worker.id,
                checkout,
                lock,
                device,
                inode,
                lock_device,
                lock_inode,
            })
        })
    }

    #[cfg(unix)]
    fn require_takeover_sessions_closed(&self, id: &str) -> Result<()> {
        use std::collections::HashSet;

        let runs = self.store.list_runs(id)?;
        let run_ids: HashSet<_> = runs.iter().map(|run| run.id.as_str()).collect();
        // A CLI can die after starting a session but before inserting its run.
        // Launch sets up this directory before any harness is allowed to start.
        let dir = self.state_dir.join("runs").join(id);
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let name = entry.context("read task run directory entry")?.file_name();
                    if let Some(name) = name.to_str()
                        && name.strip_prefix('R').is_some_and(is_decimal)
                        && !run_ids.contains(name)
                    {
                        return Err(refuse(
                            format!(
                                "unregistered run directory {name}; cannot rule out a surviving writer"
                            ),
                            vec![],
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error).context("inspect task run directories"),
        }
        for run in &runs {
            if run.ended.is_none() {
                return Err(refuse(format!("run {} has not ended", run.id), vec![]));
            }
            let session = run
                .session
                .as_ref()
                .with_context(|| format!("run {} has no verifiable session placement", run.id))?;
            if !self
                .session_runner()
                .checkout_session_gone(session)
                .with_context(|| format!("cannot verify writer from {id} {} is gone", run.id))?
            {
                return Err(refuse(
                    format!(
                        "run {} still has a live or unconfirmed writer; takeover does not stop it",
                        run.id
                    ),
                    vec![],
                ));
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    fn require_registered_path(&self, task: &Task, run: &Run) -> Result<PathBuf> {
        ensure!(
            task.id == run.task,
            "registered worker belongs to a different task"
        );
        let project = self
            .project_for(task)?
            .context("task has no configured project repository")?;
        let repo = self
            .root
            .join(project.path)
            .canonicalize()
            .context("resolve task project repository")?;
        let checkout = repo.join(".worktrees").join(&task.id);
        ensure!(
            run.worktree.as_os_str() == checkout.as_os_str(),
            "registered run is not in the configured task checkout"
        );
        Ok(repo)
    }
}

fn lock_name(id: &str) -> Result<String> {
    if !is_task_id(id) {
        return Err(refuse(
            "task checkout locking requires a T-<number> task ID",
            vec![],
        ));
    }
    Ok(format!("corral-{id}.checkout.lock"))
}

fn repo_lock_path(repo: &Path, id: &str) -> Result<PathBuf> {
    // Independent board stores using this Git repository must contend on one inode.
    let common = corral_git::git_common_dir(repo)?
        .canonicalize()
        .context("resolve common Git directory for task checkout lock")?;
    Ok(common.join(lock_name(id)?))
}

fn open_checkout_lock(path: &Path, create: bool) -> Result<File> {
    let directory = path.parent().context("task checkout lock has no parent")?;
    ensure!(
        directory
            .canonicalize()
            .context("resolve task checkout lock directory")?
            == directory,
        "task checkout lock directory must not be an alias"
    );
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open task checkout lock {}", path.display()))?;
    require_visible_lock(path, &file)?;
    Ok(file)
}

fn try_writer(path: &Path) -> Result<Option<CheckoutGuard>> {
    let file = open_checkout_lock(path, true)?;
    if !try_lock(file.try_lock_shared())? {
        return Ok(None);
    }
    require_visible_lock(path, &file)?;
    Ok(Some(CheckoutGuard::hold(file)))
}

fn is_task_id(id: &str) -> bool {
    id.strip_prefix("T-").is_some_and(is_decimal)
}

fn is_decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn try_lock(result: Result<(), TryLockError>) -> Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error).context("acquire task checkout lock"),
    }
}

fn require_visible_lock(path: &Path, file: &File) -> Result<fs::Metadata> {
    let open = file.metadata().context("stat open task checkout lock")?;
    let visible = fs::symlink_metadata(path).context("stat visible task checkout lock")?;
    ensure!(
        open.is_file() && visible.is_file(),
        "task checkout lock is not a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        ensure!(
            open.nlink() == 1
                && visible.nlink() == 1
                && open.dev() == visible.dev()
                && open.ino() == visible.ino(),
            "task checkout lock was replaced or aliased"
        );
    }
    Ok(open)
}

#[cfg(unix)]
fn file_identity(meta: &fs::Metadata) -> (String, String) {
    use std::os::unix::fs::MetadataExt as _;
    (meta.dev().to_string(), meta.ino().to_string())
}
