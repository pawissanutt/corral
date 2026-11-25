use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result};
use serde_json::json;

use crate::{
    Actor, Blocked, CheckKind, CheckResult, Engine, Next, Op, PipelineMode, Status, Store, Task,
    as_refusal, checkout::Checkout, events::EventKind, review::gate_notes,
    tasks::next_force_approve,
};

pub(crate) const GATE_FAILED: &str = "gate failed";
pub(crate) const PIPELINE_FAILED: &str = "pipeline failed";

pub(crate) struct OperationFailure<'a> {
    pub(crate) operation: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) error: &'a anyhow::Error,
    pub(crate) next: Vec<Next>,
}

impl Engine {
    pub fn advance(&self, id: &str) -> Result<()> {
        let task = self.store.require_task(id)?;
        if task.status != Status::Review {
            return self.clean_terminal_pipeline_lock(&task);
        }
        let Some(_lock) = self.pipeline_lock(id, false, true)? else {
            return Ok(());
        };
        let mut effects_started = false;
        let result = self.advance_locked(id, &mut effects_started);
        if let Err(error) = &result
            && (effects_started || as_refusal(error).is_none())
        {
            self.store.transaction(|store| {
                let mut task = store.require_task(id)?;
                if task.status == Status::Review {
                    self.record_operation_failure(
                        store,
                        &mut task,
                        None,
                        OperationFailure {
                            operation: "pipeline",
                            reason: PIPELINE_FAILED,
                            error,
                            next: vec![
                                Next::run(Op::Gate { id: id.into() }),
                                next_force_approve(id),
                            ],
                        },
                        Actor::Auto,
                    )?;
                }
                Ok(())
            })?;
        }
        result.and(self.unlink_terminal_pipeline_lock(id))
    }

    /// Persist an observable outcome even when the submitting caller no longer waits.
    pub(crate) fn record_operation_failure(
        &self,
        store: &Store,
        task: &mut Task,
        run: Option<&str>,
        failure: OperationFailure<'_>,
        actor: Actor,
    ) -> Result<()> {
        let detail = format!("{:#}", failure.error);
        self.event(
            store,
            Some(&task.id),
            run,
            EventKind::Error,
            actor,
            json!({"operation": failure.operation, "reason": detail}),
        )?;
        self.block_in(
            store,
            task,
            run,
            Blocked {
                reason: failure.reason.into(),
                detail: Some(detail),
                next: failure.next,
            },
            actor,
        )
    }

    fn advance_locked(&self, id: &str, effects_started: &mut bool) -> Result<()> {
        let task = self.store.require_task(id)?;
        if task.status != Status::Review {
            return Ok(());
        }
        if self.advance_queued_repair(id)? {
            return Ok(());
        }
        let worker = self.last_worker(&self.store, &task)?;
        let checkout = Checkout::from_run(&worker)?;
        let head = corral_git::rev_sha(&checkout.repo, &checkout.branch_ref())
            .context("read pipeline branch HEAD")?;
        let project = self.project_for(&task)?;
        if let Some(project) = project.as_ref().filter(|p| !p.gate.is_empty()) {
            let check = match self.store.latest_check(id, CheckKind::Gate, &head)? {
                Some(check) => check,
                None => self.run_gate(
                    &worker,
                    task.round,
                    Some(project),
                    Actor::Auto,
                    effects_started,
                )?,
            };
            if check.result == CheckResult::Fail {
                return self.send_back(id, &gate_notes(&check));
            }
        }
        let task = self.store.require_task(id)?;
        if task.status != Status::Review {
            return Ok(());
        }
        let review = if self.store.has_verdict_since_done(id, task.round)? {
            self.store
                .latest_check(id, CheckKind::Review, &task.round.to_string())?
        } else {
            None
        };
        if review.is_none() && project.as_ref().is_some_and(|p| p.reviewer.is_some()) {
            // review_as requests the current round at most once per live reviewer.
            self.review_as(id, None, Actor::Auto)?;
            return Ok(());
        }
        if review.is_some_and(|check| check.result == CheckResult::Fail) {
            return Ok(());
        }
        if project
            .as_ref()
            .map_or(self.config.defaults.auto_land, |p| p.auto_land)
        {
            self.approve_as(id, None, false, Actor::Auto)?;
            return Ok(());
        }
        self.store.transaction(|store| {
            if store.require_task(id)?.status == Status::Review
                && !store.has_ready_since_done(id)?
            {
                self.event(
                    store,
                    Some(id),
                    Some(&worker.id),
                    EventKind::Ready,
                    Actor::Auto,
                    json!({"sha":head}),
                )?;
            }
            Ok(())
        })
    }

    pub(crate) fn kick_pipeline(&self, id: &str) -> Result<()> {
        match self.pipeline {
            PipelineMode::Inline => self.advance(id),
            PipelineMode::Off => Ok(()),
            PipelineMode::Spawn => {
                let mut command = Command::new(crate::binary_path()?);
                command
                    .arg("--root")
                    .arg(&self.root)
                    .args(["pipeline", id])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::runner::detach(&mut command)?;
                let mut child = command
                    .spawn()
                    .with_context(|| format!("spawn detached pipeline for {id}"))?;
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(())
            }
        }
    }

    /// Serialize owner gate and landing calls with the same task's pipeline.
    pub(crate) fn with_pipeline_lock<T>(
        &self,
        id: &str,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.store.require_task(id)?;
        let _lock = self
            .pipeline_lock(id, true, true)?
            .context("acquire task pipeline lock")?;
        let result = operation();
        let cleanup = self.unlink_terminal_pipeline_lock(id);
        match result {
            Ok(value) => cleanup.map(|()| value),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn clean_terminal_pipeline_lock(&self, task: &Task) -> Result<()> {
        if matches!(task.status, Status::Done | Status::Dropped)
            && let Some(_lock) = self.pipeline_lock(&task.id, false, false)?
        {
            self.unlink_terminal_pipeline_lock(&task.id)?;
        }
        Ok(())
    }

    fn unlink_terminal_pipeline_lock(&self, id: &str) -> Result<()> {
        if matches!(
            self.store.require_task(id)?.status,
            Status::Done | Status::Dropped
        ) {
            match fs::remove_file(self.pipeline_path(id)) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error).context("remove terminal task pipeline lock"),
            }
        }
        Ok(())
    }

    fn pipeline_path(&self, id: &str) -> PathBuf {
        self.state_dir.join("locks").join(format!("{id}.pipeline"))
    }

    fn pipeline_lock(&self, id: &str, wait: bool, create: bool) -> Result<Option<File>> {
        let path = self.pipeline_path(id);
        loop {
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .create(create)
                .truncate(false)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("open pipeline lock {}", path.display()));
                }
            };
            if wait {
                file.lock().context("lock task pipeline")?;
            } else {
                match file.try_lock() {
                    Ok(()) => (),
                    Err(TryLockError::WouldBlock) => return Ok(None),
                    Err(TryLockError::Error(error)) => {
                        return Err(error).context("try to lock task pipeline");
                    }
                }
            }
            if current_lock_file(&file, &path)? {
                return Ok(Some(file));
            }
            // A terminal transition may have unlinked the old file while we were opening or waiting on it.
            if !wait {
                return Ok(None);
            }
        }
    }
}

#[cfg(unix)]
fn current_lock_file(file: &File, path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    let open = file.metadata().context("stat opened pipeline lock")?;
    match fs::metadata(path) {
        Ok(visible) => Ok(open.dev() == visible.dev() && open.ino() == visible.ino()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("stat visible pipeline lock"),
    }
}

#[cfg(not(unix))]
fn current_lock_file(_file: &File, path: &Path) -> Result<bool> {
    path.try_exists().context("inspect visible pipeline lock")
}
