//! Compatibility for send-backs queued before Review held a slot. Resume
//! these with their original notes; new send-backs never wait for capacity.
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use crate::{Actor, Engine, Next, Op, Status, Store, pipeline::OperationFailure, review::SendBack};

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
    pub(crate) fn advance_queued_repair(&self, id: &str) -> Result<bool> {
        if !self.store.has_queued_repair(id)? {
            return Ok(false);
        }
        let mut attempted = None;
        let result = self.launch_transaction(id, |store, pending| {
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
