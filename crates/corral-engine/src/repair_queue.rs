//! A review releases its worker slot. Persist automatic send-backs until the
//! same write transaction can both admit the repair and resume its worker.
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use crate::{Engine, Status, Store, Task, config::EffectiveProject, review::SendBack};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pending_repairs (
    task TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    notes TEXT NOT NULL, onto TEXT
);
CREATE TRIGGER IF NOT EXISTS clear_pending_repair AFTER UPDATE OF status ON tasks
    WHEN OLD.status='review' AND NEW.status!='review'
    BEGIN DELETE FROM pending_repairs WHERE task=NEW.id; END;";

impl Store {
    pub(crate) fn queue_repair(&self, id: &str, notes: &str, onto: Option<&str>) -> Result<()> {
        self.connection.execute(
            "INSERT INTO pending_repairs(task,notes,onto) VALUES(?1,?2,?3) ON CONFLICT(task) DO UPDATE SET notes=excluded.notes,onto=excluded.onto",
            rusqlite::params![id, notes, onto],
        ).context("queue repair waiting for a worker slot")?;
        Ok(())
    }

    pub(crate) fn has_queued_repair(&self, id: &str) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_repairs WHERE task=?1)",
                [id],
                |row| row.get(0),
            )
            .context("check for queued repair")
    }

    fn repair(&self, id: &str) -> Result<Option<(String, Option<String>)>> {
        self.connection
            .query_row(
                "SELECT notes,onto FROM pending_repairs WHERE task=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("read pending repair")
    }

    pub(crate) fn queued_repairs(&self) -> Result<Vec<String>> {
        let mut query = self
            .connection
            .prepare("SELECT task FROM pending_repairs ORDER BY rowid")
            .context("prepare repair queue")?;
        query
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()
            .context("list queued repairs")
    }
}

impl Engine {
    pub(crate) fn worker_slot_full(
        &self,
        store: &Store,
        task: &Task,
        project: Option<&EffectiveProject>,
    ) -> Result<bool> {
        if matches!(task.status, Status::Blocked | Status::Running) {
            return Ok(false); // Already counted by occupied_slots.
        }
        if store.occupied_slots(None)? >= self.config.limits.running {
            return Ok(true);
        }
        if let Some(limit) = project.and_then(|p| p.running) {
            return Ok(store.occupied_slots(task.project.as_deref())? >= limit);
        }
        Ok(false)
    }

    pub(crate) fn advance_queued_repair(&self, id: &str) -> Result<bool> {
        if !self.store.has_queued_repair(id)? {
            return Ok(false);
        }
        self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if task.status == Status::Review
                && let Some((notes, onto)) = store.repair(id)?
            {
                let project = self.project_for(&task)?;
                if !self.worker_slot_full(store, &task, project.as_ref())? {
                    self.send_back_in(
                        store,
                        pending,
                        &mut task,
                        &notes,
                        SendBack::Automatic {
                            onto: onto.as_deref(),
                        },
                    )?;
                }
            }
            Ok(true)
        })
    }
}
