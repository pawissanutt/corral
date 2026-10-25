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
        self.fixture_write_events(&input[..end], false, false)?;
        *pending = input[end..].to_owned();
        Ok(())
    }

    pub(crate) fn fixture_replace_events(&self, jsonl: &str) -> Result<()> {
        let mut pending = self
            .fixture_event_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.fixture_write_events(jsonl, true, false)?;
        pending.clear();
        Ok(())
    }

    pub(crate) fn fixture_break_events(&self) -> Result<()> {
        let mut pending = self
            .fixture_event_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.fixture_write_events("", false, true)?;
        pending.clear();
        Ok(())
    }

    fn fixture_write_events(&self, jsonl: &str, replace: bool, corrupt: bool) -> Result<()> {
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
                 DELETE FROM events;",
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
        if corrupt {
            // Unlike an imported malformed line, this row claims to be a valid document.
            // Real row decoding must fail, without a flag or alternate reader.
            let changed = transaction
                .execute("UPDATE events SET detail = '{', malformed = 0", [])
                .context("corrupt existing fixture event documents")?;
            if changed == 0 {
                transaction
                    .execute(
                        "INSERT INTO events(seq, ts, kind, detail, malformed)
                 VALUES (1, ?1, 'fixture_corrupted', '{', 0)",
                        params![codec::ts(now)],
                    )
                    .context("corrupt fixture event document")?;
            }
        }
        transaction.commit().context("commit fixture events")
    }
}
