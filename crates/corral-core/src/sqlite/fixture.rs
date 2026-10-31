//! Explicit fixture mutations of the canonical event tables, never a reader routing mode.

use anyhow::{Context, Result};
use rusqlite::{TransactionBehavior, params};

use super::{Hooks, SqliteBackend, codec, tx};
use crate::backend::{ImportBatch, ImportedEvent, NewEvent, OpKind, Operation};

impl SqliteBackend {
    pub(crate) fn fixture_append_events(&self, jsonl: &str) -> Result<()> {
        let mut pending = self
            .fixture_event_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input = format!("{pending}{jsonl}");
        let Some(end) = input.rfind('\n').map(|index| index + 1) else {
            *pending = input;
            return Ok(());
        };
        self.fixture_write_events(&input[..end], false)?;
        *pending = input[end..].to_owned();
        Ok(())
    }

    pub(crate) fn fixture_replace_events(&self, jsonl: &str) -> Result<()> {
        let mut pending = self
            .fixture_event_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.fixture_write_events(jsonl, true)?;
        pending.clear();
        Ok(())
    }

    /// A real record-publication failure visible to independently opened fixture processes.
    /// Triggers affect only the named row, never reads; production has no routing/fault mode.
    pub(crate) fn fixture_block_record(&self, area: &str, key: &str, blocked: bool) -> Result<()> {
        use sha2::{Digest, Sha256};
        let tag = format!("{:x}", Sha256::digest(serde_json::to_vec(&(area, key))?));
        let mut conn = self.writer();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin fixture record publication fault")?;
        let quoted_area: String = tx
            .query_row("SELECT quote(?1)", params![area], |row| row.get(0))
            .context("quote fixture record area")?;
        let quoted_key: String = tx
            .query_row("SELECT quote(?1)", params![key], |row| row.get(0))
            .context("quote fixture record key")?;
        for event in ["INSERT", "UPDATE"] {
            let name = format!("fixture_record_fault_{tag}_{event}");
            tx.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}"))
                .context("restore fixture record publication")?;
            if blocked {
                tx.execute_batch(&format!(
                    "CREATE TRIGGER {name} BEFORE {event} ON records
                    WHEN NEW.area={quoted_area} AND NEW.key={quoted_key}
                    BEGIN SELECT RAISE(ABORT, 'fixture_record_publication_blocked'); END"
                ))
                .context("block exact fixture record publication")?;
            }
        }
        tx.commit()
            .context("commit fixture record publication fault")
    }

    pub(crate) fn fixture_break_events(&self) -> Result<()> {
        let mut pending = self
            .fixture_event_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.writer()
            .execute_batch("ALTER TABLE events RENAME TO fixture_events_unavailable")
            .context("make fixture event storage unavailable")?;
        pending.clear();
        Ok(())
    }

    pub(crate) fn fixture_restore_events(&self) -> Result<()> {
        let mut conn = self.writer();
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin fixture event restore")?;
        restore_event_table(&transaction)?;
        transaction
            .commit()
            .context("restore fixture event storage")
    }

    fn fixture_write_events(&self, jsonl: &str, replace: bool) -> Result<()> {
        let now = self.options.clock.now();
        let events = jsonl
            .lines()
            .map(|line| match serde_json::from_str::<crate::Event>(line) {
                Ok(event) => Ok(ImportedEvent {
                    event: NewEvent {
                        ts: event.ts,
                        task: event.task,
                        kind: event.kind,
                        actor: event
                            .actor
                            .map(serde_json::to_value)
                            .transpose()?
                            .and_then(|actor| actor.as_str().map(str::to_owned)),
                        via: event.via,
                        run: None,
                        detail: event.detail,
                    },
                    malformed: false,
                }),
                Err(_) => Ok(ImportedEvent {
                    event: NewEvent {
                        ts: now,
                        task: None,
                        kind: "malformed".to_owned(),
                        actor: None,
                        via: None,
                        run: None,
                        detail: line.into(),
                    },
                    malformed: true,
                }),
            })
            .collect::<Result<Vec<_>>>()?;
        let mut conn = self.writer();
        let transaction = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin fixture event transaction")?;
        restore_event_table(&transaction)?;
        if replace {
            // Condition pointers and blob owners reference event identities. Remove those
            // references in the same transaction, retaining unrelated task/blob state.
            transaction
                .execute(
                    "UPDATE blobs SET
                   refs = refs - (SELECT COUNT(*) FROM blob_refs
                     WHERE owner_kind = 'event' AND blob = blobs.hash),
                   unreferenced_since = CASE WHEN refs = (SELECT COUNT(*) FROM blob_refs
                     WHERE owner_kind = 'event' AND blob = blobs.hash)
                     THEN COALESCE(unreferenced_since, ?1)
                     ELSE unreferenced_since END
                 WHERE hash IN (SELECT blob FROM blob_refs WHERE owner_kind = 'event')",
                    params![codec::ts(now)],
                )
                .context("release fixture event blob references")?;
            transaction
                .execute_batch(
                    "DELETE FROM conditions;
                 DELETE FROM condition_runs;
                 DELETE FROM blob_refs WHERE owner_kind = 'event';
                 DELETE FROM events;
                 DELETE FROM lesson_approvals;",
                )
                .context("clear fixture events and their references")?;
        }
        let hooks = Hooks {
            faults: std::sync::Arc::clone(&self.options.faults),
            barrier: self.options.barrier.clone(),
            op: OpKind::ImportBatch,
        };
        tx::run(
            &transaction,
            Operation::ImportBatch(ImportBatch {
                blobs: Vec::new(),
                tasks: Vec::new(),
                events,
                records: Vec::new(),
            }),
            now,
            &hooks,
        )
        .context("import fixture events")?;
        transaction.commit().context("commit fixture events")
    }
}

fn restore_event_table(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let unavailable: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'fixture_events_unavailable')",
        [], |row| row.get(0),
    ).context("inspect fixture event storage")?;
    if unavailable {
        transaction
            .execute_batch("ALTER TABLE fixture_events_unavailable RENAME TO events")
            .context("restore fixture event table")?;
    }
    Ok(())
}
