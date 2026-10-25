//! Test and benchmark fixture hooks: they store exact states the facade's writers would
//! normalize (preset timestamps, legacy or malformed documents, imported history), so
//! fixtures never depend on the storage layout.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Store;
use crate::model::Task;

impl Store {
    fn event_fixture(&self) -> Result<&crate::sqlite::SqliteBackend> {
        self.db
            .sqlite
            .as_ref()
            .context("this store has no database file")
    }

    /// Imports completed JSONL lines into the database. An unterminated suffix is fixture
    /// staging input only: readers see it only after a later append completes the line.
    #[doc(hidden)]
    pub fn fixture_append_events(&self, jsonl: &str) -> Result<()> {
        self.event_fixture()?.fixture_append_events(jsonl)
    }

    /// Replaces database event history with imported JSONL, including a final EOF line.
    /// This resets fixture history; production event streams remain immutable.
    #[doc(hidden)]
    pub fn fixture_replace_events(&self, jsonl: &str) -> Result<()> {
        self.event_fixture()?.fixture_replace_events(jsonl)
    }

    /// Clears database event history and any incomplete fixture input.
    #[doc(hidden)]
    pub fn fixture_remove_events(&self) -> Result<()> {
        self.event_fixture()?.fixture_replace_events("")
    }

    /// Makes event reads and appends fail without changing existing event data.
    #[doc(hidden)]
    pub fn fixture_break_events(&self) -> Result<()> {
        self.event_fixture()?.fixture_break_events()
    }

    /// Restores event storage after [`Store::fixture_break_events`], preserving its data.
    #[doc(hidden)]
    pub fn fixture_restore_events(&self) -> Result<()> {
        self.event_fixture()?.fixture_restore_events()
    }

    /// Stores exactly this task: no timestamp, scope check or event, unlike [`Store::save`].
    #[doc(hidden)]
    pub fn fixture_put_task(&self, task: &Task) -> Result<()> {
        let id = Store::normalize_id(&task.id)?;
        match self.read_stored(&id)? {
            Some(stored) => {
                let record = crate::TaskRecord {
                    task: task.clone(),
                    recoveries: stored.record.recoveries.clone(),
                };
                self.write_record(&stored, &record, true).map(drop)
            }
            None => self.insert_records(&[crate::TaskRecord {
                task: task.clone(),
                recoveries: Vec::new(),
            }]),
        }
    }

    /// Stores the task a legacy task document describes (schema 2 or 3, recovery evidence
    /// included) exactly; a document that does not parse is refused.
    #[doc(hidden)]
    pub fn fixture_put_task_text(&self, file_stem: &str, text: &str) -> Result<PathBuf> {
        let record = crate::legacy::parse_task_record(text, Path::new(file_stem))?;
        let id = Store::normalize_id(&record.task.id)?;
        match self.read_stored(&id)? {
            Some(stored) => {
                self.write_record(&stored, &record, true)?;
            }
            None => self.insert_records(std::slice::from_ref(&record))?,
        }
        Ok(PathBuf::from(file_stem))
    }

    /// Makes [`Store::publish_staged`] refuse to publish `id` until unblocked, as an
    /// interrupted publication would leave it.
    #[doc(hidden)]
    pub fn fixture_block_task(&self, id: &str, blocked: bool) -> Result<()> {
        let id = Store::normalize_id(id)?;
        let mut set = self
            .db
            .blocked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if blocked {
            set.insert(id);
        } else {
            set.remove(&id);
        }
        Ok(())
    }

    /// Deletes a task without any event, as a lost or foreign deletion would.
    #[doc(hidden)]
    pub fn fixture_remove_task(&self, id: &str) -> Result<()> {
        let id = Store::normalize_id(id)?;
        let sqlite = self
            .db
            .sqlite
            .as_ref()
            .context("this store has no database file")?;
        sqlite.fixture_delete_task(crate::backend::TaskNum(super::id_number(&id)))
    }

    /// Corrupts a stored task's head state so it no longer decodes; `detail` makes the decode
    /// error distinct.
    #[doc(hidden)]
    pub fn fixture_corrupt_task(&self, id: &str, detail: &str) -> Result<()> {
        let id = Store::normalize_id(id)?;
        let sqlite = self
            .db
            .sqlite
            .as_ref()
            .context("this store has no database file")?;
        let state = serde_json::json!({"state": {"status": detail}}).to_string();
        sqlite.fixture_raw_task_state(crate::backend::TaskNum(super::id_number(&id)), &state)
    }

    /// The stored record of a task in the legacy Markdown form (schema version and recovery
    /// evidence included).
    #[doc(hidden)]
    pub fn fixture_task_text(&self, id: &str) -> Result<String> {
        crate::legacy::render_task_record(&self.task_record(id)?)
    }
}
