//! Maintained eligible-artifact ordering. Reads acquire only the selected documents.
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::Value;

use super::{codec::*, load};
use crate::backend::{HistoryRow, TaskNum, lesson_artifacts::metadata};

pub const TABLES: &str = "
CREATE TABLE lesson_artifacts(task INTEGER NOT NULL, key TEXT NOT NULL,
    added TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(task, key)) STRICT;
CREATE INDEX lesson_artifacts_order ON lesson_artifacts(task, added DESC, path DESC, key);
";

#[cfg(test)]
#[path = "lesson_artifacts_tests.rs"]
mod tests;

/// Backfill existing databases once, before any evaluation opens its read view.
pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin lesson artifact initialization")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'lesson_artifacts')",
            [],
            |r| r.get(0),
        )
        .context("check lesson artifact schema")?;
    if exists {
        return Ok(());
    }
    tx.execute_batch(TABLES)
        .context("create lesson artifact index")?;
    for (task, key, doc) in load::all(
        &tx,
        "SELECT task, key, doc FROM task_rows WHERE part = 'artifact'",
        [],
        |r| {
            Ok((
                TaskNum(load::num(r, 0)?),
                load::text(r, 1)?,
                load::doc(r, 2)?,
            ))
        },
    )? {
        put(&tx, task, &key, Some(&doc))?;
    }
    tx.commit().context("commit lesson artifact initialization")
}

pub(super) fn put(db: &Connection, task: TaskNum, key: &str, doc: Option<&Value>) -> Result<()> {
    db.execute(
        "DELETE FROM lesson_artifacts WHERE task = ?1 AND key = ?2",
        params![int(task.0)?, key],
    )
    .context("remove old lesson artifact metadata")?;
    if let Some((added, path)) = doc.map(metadata).transpose()?.flatten() {
        db.execute(
            "INSERT INTO lesson_artifacts(task, key, added, path) VALUES (?1, ?2, ?3, ?4)",
            params![int(task.0)?, key, ts(added), path],
        )
        .context("index lesson artifact")?;
    }
    Ok(())
}

pub(super) fn read(db: &Connection, task: TaskNum, limit: u32) -> Result<Vec<HistoryRow>> {
    // CROSS JOIN fixes the indexed projection as the outer loop; no history-wide sort.
    load::all(
        db,
        "SELECT r.part, r.key, r.current, r.open, r.doc, r.idx
         FROM (SELECT key, added, path FROM lesson_artifacts WHERE task = ?1
               ORDER BY added DESC, path DESC, key LIMIT ?2) selected
         CROSS JOIN task_rows r
         WHERE r.task = ?1 AND r.part = 'artifact' AND r.key = selected.key
         ORDER BY selected.added DESC, selected.path DESC, selected.key",
        params![int(task.0)?, i64::from(limit)],
        load::history_row,
    )
}
