//! Test and benchmark fixture hooks: they store exact states the facade's writers would
//! normalize (preset timestamps, legacy or malformed documents, imported history), so
//! fixtures never depend on the storage layout.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::{Store, atomic_write, file_stem, render_task};
use crate::model::Task;

impl Store {
    /// Appends JSONL event lines verbatim, as the legacy import does; malformed or
    /// unterminated lines are kept and skipped by readers.
    #[doc(hidden)]
    pub fn fixture_append_events(&self, jsonl: &str) -> Result<()> {
        let path = self.events_path();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.write_all(jsonl.as_bytes())
            .with_context(|| format!("appending to {}", path.display()))
    }

    /// Replaces the whole event history with these JSONL lines, verbatim.
    #[doc(hidden)]
    pub fn fixture_replace_events(&self, jsonl: &str) -> Result<()> {
        let path = self.events_path();
        fs::write(&path, jsonl).with_context(|| format!("writing {}", path.display()))
    }

    /// Makes the event history unreadable.
    #[doc(hidden)]
    pub fn fixture_break_events(&self) -> Result<()> {
        let path = self.events_path();
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        fs::create_dir(&path).with_context(|| format!("creating {}", path.display()))
    }

    /// Stores exactly this task: no timestamp, scope check or event, unlike [`Store::save`].
    #[doc(hidden)]
    pub fn fixture_put_task(&self, task: &Task) -> Result<()> {
        let path = if task.path.as_os_str().is_empty() {
            self.task_location(&file_stem(&task.id, &task.title))
        } else {
            task.path.clone()
        };
        atomic_write(&path, &render_task(task))
    }

    /// Stores a raw task document (legacy or malformed) under `file_stem`.
    #[doc(hidden)]
    pub fn fixture_put_task_text(&self, file_stem: &str, text: &str) -> Result<PathBuf> {
        let path = self.task_location(file_stem);
        let dir = self.dir.join("tasks");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Deletes a task's stored document without any event, as a lost or foreign deletion would.
    #[doc(hidden)]
    pub fn fixture_remove_task(&self, id: &str) -> Result<()> {
        let path = self.get(id)?.path;
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
    }

    /// The stored document of a task, as persisted (schema version and recovery evidence
    /// included).
    #[doc(hidden)]
    pub fn fixture_task_text(&self, id: &str) -> Result<String> {
        let path = self.get(id)?.path;
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }
}
