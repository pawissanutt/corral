//! Batch allocation and publication under the same lock as ordinary task creation.
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use jiff::Timestamp;

use super::{Store, file_id_number, file_stem, render_task};
use crate::{Origin, Status, Task};

impl Store {
    /// Allocate drafts, initialize and validate them, then publish tasks and snapshots.
    ///
    /// The callback runs against fresh configuration under the board lock and must
    /// not call store write methods. Snapshot paths are relative to `.corral/`.
    /// Preview runs take no locks and write nothing. Ordinary publication errors
    /// roll back every new file and directory before releasing the lock.
    pub fn create_batch(
        &self,
        count: usize,
        dry_run: bool,
        initialize: impl FnOnce(&Store, &mut [Task]) -> Result<Vec<(PathBuf, Vec<u8>)>>,
    ) -> Result<Vec<Task>> {
        self.check_runtime()?;
        ensure!(count > 0, "empty task batch");
        let _board = if dry_run {
            None
        } else {
            Some(self.lock("dispatch")?)
        };
        let fresh = self.reopen_unobserved()?;
        let first = fresh
            .task_files()?
            .iter()
            .filter_map(|path| file_id_number(path))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .context("task ID exhausted")?;
        let mut tasks = (0..count)
            .map(|index| {
                let number = first
                    .checked_add(u64::try_from(index)?)
                    .context("task ID exhausted")?;
                Ok(draft_task(number, "", None, Status::Backlog, ""))
            })
            .collect::<Result<Vec<_>>>()?;
        let snapshots = initialize(&fresh, &mut tasks)?;
        for task in &mut tasks {
            task.path = fresh
                .dir()
                .join("tasks")
                .join(format!("{}.md", file_stem(&task.id, &task.title)));
        }
        if dry_run {
            return Ok(tasks);
        }
        let stage = tempfile::tempdir_in(fresh.dir()).context("staging task batch")?;
        for task in &mut tasks {
            let now = Timestamp::now();
            super::normalize::task(task, None, now)?;
            task.updated = now;
        }
        let files = tasks
            .iter()
            .map(|task| (task.path.clone(), render_task(task).into_bytes()))
            .chain(
                snapshots
                    .into_iter()
                    .map(|(path, bytes)| (fresh.dir().join(path), bytes)),
            );
        let mut staged = Vec::new();
        for (path, bytes) in files {
            let mut file =
                tempfile::NamedTempFile::new_in(stage.path()).context("staging batch file")?;
            file.write_all(&bytes)
                .with_context(|| format!("staging {}", path.display()))?;
            staged.push((path, file));
        }
        let mut publication = Publication::default();
        for (path, file) in staged {
            let mut missing = Vec::new();
            let mut parent = path.parent().context("batch path has no parent")?;
            while !parent.exists() {
                missing.push(parent.to_path_buf());
                parent = parent.parent().context("batch directory has no parent")?;
            }
            for directory in missing.into_iter().rev() {
                fs::create_dir(&directory)
                    .with_context(|| format!("creating {}", directory.display()))?;
                publication.directories.push(directory);
            }
            file.persist_noclobber(&path)
                .with_context(|| format!("publishing {}", path.display()))?;
            publication.files.push(path);
        }
        fresh.log_event(
            Some(&tasks[0].id),
            "imported",
            serde_json::json!({"tasks": tasks.iter().map(|t| &t.id).collect::<Vec<_>>()}),
        )?;
        publication.committed = true;
        Ok(tasks)
    }
}

#[derive(Default)]
struct Publication {
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    committed: bool,
}

impl Drop for Publication {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in self.files.iter().rev() {
            if let Err(error) = fs::remove_file(path) {
                crate::messages::report_file(path, format!("rolling back import: {error}"));
            }
        }
        for path in self.directories.iter().rev() {
            if let Err(error) = fs::remove_dir(path) {
                crate::messages::report_file(
                    path,
                    format!("rolling back import directory: {error}"),
                );
            }
        }
    }
}

pub(super) fn draft_task(
    number: u64,
    title: &str,
    project: Option<&str>,
    status: Status,
    body: &str,
) -> Task {
    let now = Timestamp::now();
    Task {
        park: None,
        landed: None,
        infra_wait: None,
        attention: Default::default(),
        dashboard: None,
        origin: Some(if status == Status::Idea {
            Origin::Owner
        } else {
            Origin::Agent
        }),
        idea_decision: None,
        comments: Vec::new(),
        completion: None,
        result: None,
        id: format!("T-{number:03}"),
        title: title.to_owned(),
        status,
        status_since: None,
        project: project.map(str::to_owned),
        kind: None,
        size: None,
        priority: "P2".into(),
        depends_on: Vec::new(),
        labels: Vec::new(),
        write_scope: Vec::new(),
        profile: None,
        reviewer: None,
        queued: false,
        dispatch_plan: None,
        launch: None,
        dispatch_hold: false,
        review_reservation: None,
        review_wait: None,
        review: None,
        plan_required: None,
        design: false,
        plan: None,
        tracker: false,
        approved_scope: None,
        approved_scope_revision: 0,
        approved_scope_history: Vec::new(),
        member_gates: Default::default(),
        aggregate_attempts: Vec::new(),
        parent: None,
        source_digest: None,
        children: Vec::new(),
        plan_runs: Vec::new(),
        plans: Vec::new(),
        rounds: 0,
        created: now,
        updated: now,
        runs: Vec::new(),
        artifacts: Vec::new(),
        agent_origin: None,
        agent_pm_receipts: Vec::new(),
        body: body.to_owned(),
        path: PathBuf::new(),
    }
}
