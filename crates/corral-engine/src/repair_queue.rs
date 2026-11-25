//! A review releases its worker slot. Persist automatic send-backs until the
//! same write transaction can both admit the repair and resume its worker.
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use crate::{
    Actor, Engine, Next, Op, Status, Store, Task, config::EffectiveProject,
    pipeline::OperationFailure, review::SendBack,
};

pub(crate) const EXHAUSTED: &str = "automatic change rounds exhausted";
const REPAIR_FAILED: &str = "queued repair failed";

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
            "INSERT INTO pending_repairs(task,notes,onto) VALUES(?1,?2,?3) ON CONFLICT(task) DO UPDATE SET notes=excluded.notes,onto=excluded.onto WHERE notes IS NOT excluded.notes OR onto IS NOT excluded.onto",
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

    fn task_event_seq(&self, id: &str) -> Result<i64> {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(seq),0) FROM events WHERE task=?1",
                [id],
                |row| row.get(0),
            )
            .context("read task event sequence for queued repair")
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
        let repair_block = task.status == Status::Blocked
            && task
                .blocked
                .as_ref()
                .is_some_and(|b| matches!(b.reason.as_str(), EXHAUSTED | REPAIR_FAILED));
        if task.status == Status::Running || (task.status == Status::Blocked && !repair_block) {
            return Ok(false); // Already admitted.
        }
        let occupied =
            |project: Option<&str>| -> Result<usize> {
                if !repair_block {
                    return store.occupied_slots(project);
                }
                // Fresh dispatch still counts every block. When explicitly resuming
                // a repair, don't count other repair decisions that never started a
                // worker: two exhausted reviews must not deadlock a one-slot board.
                let count: i64 = store.connection.query_row(
                "SELECT count(*) FROM tasks WHERE (status='running' OR (status='blocked' AND
                 COALESCE(json_extract(document,'$.blocked.reason'),'') NOT IN (?2,?3)))
                 AND (?1 IS NULL OR project=?1)",
                rusqlite::params![project, EXHAUSTED, REPAIR_FAILED], |row| row.get(0)
            ).context("count slots for owner repair")?;
                count.try_into().context("invalid repair slot count")
            };
        if occupied(None)? >= self.config.limits.running {
            return Ok(true);
        }
        if let Some(limit) = project.and_then(|p| p.running) {
            return Ok(occupied(task.project.as_deref())? >= limit);
        }
        Ok(false)
    }

    pub(crate) fn advance_queued_repair(&self, id: &str) -> Result<bool> {
        if !self.store.has_queued_repair(id)? {
            return Ok(false);
        }
        let mut attempted = None;
        let result = self.launch_transaction(|store, pending| {
            let mut task = store.require_task(id)?;
            if task.status == Status::Review
                && let Some(repair) = store.repair(id)?
            {
                let (_, (notes, onto)) = attempted.insert((store.task_event_seq(id)?, repair));
                self.send_back_in(
                    store,
                    pending,
                    &mut task,
                    notes,
                    SendBack::Automatic {
                        onto: onto.as_deref(),
                    },
                )?;
            }
            Ok(true)
        });
        if let Err(error) = &result
            && let Some((seq, attempted)) = attempted
        {
            // The failed launch and its effects must roll back first. Never
            // apply its refusal to a newer verdict or owner transition.
            self.store.transaction(|store| {
                let mut task = store.require_task(id)?;
                if task.status == Status::Review
                    && store.task_event_seq(id)? == seq
                    && store.repair(id)?.as_ref() == Some(&attempted)
                {
                    let retry = Next::run(Op::Retry {
                        id: id.into(),
                        profile: None,
                        fresh: false,
                        note: Some(attempted.0),
                    });
                    self.record_operation_failure(
                        store,
                        &mut task,
                        None,
                        OperationFailure {
                            operation: "queued repair",
                            reason: REPAIR_FAILED,
                            error,
                            next: vec![retry],
                        },
                        Actor::Auto,
                    )?;
                }
                Ok(())
            })?;
        }
        result
    }
}
