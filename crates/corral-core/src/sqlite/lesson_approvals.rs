//! Approval facts maintained atomically with their events and read on the same pinned view.
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior, params};

use super::{codec::*, load};
use crate::backend::{LessonApproval, NewEvent, lesson_approvals::attribution};

pub const TABLES: &str = "
CREATE TABLE lesson_approvals(sha TEXT NOT NULL, task TEXT NOT NULL, run TEXT NOT NULL,
    PRIMARY KEY(sha, task, run)) STRICT;
";

pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin lesson approval initialization")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'lesson_approvals')",
            [],
            |r| r.get(0),
        )
        .context("check lesson approval schema")?;
    if exists {
        return Ok(());
    }
    tx.execute_batch(TABLES)
        .context("create lesson approval index")?;
    for event in load::all(
        &tx,
        "SELECT ts, task, kind, actor, via, run, detail FROM events
         WHERE kind = 'approved' AND malformed = 0 ORDER BY seq",
        [],
        |r| {
            Ok(NewEvent {
                ts: load::time(r, 0)?,
                task: load::opt_text(r, 1)?,
                kind: load::text(r, 2)?,
                actor: load::opt_text(r, 3)?,
                via: load::opt_text(r, 4)?,
                run: load::opt_text(r, 5)?,
                detail: load::doc(r, 6)?,
            })
        },
    )? {
        put(&tx, &event)?;
    }
    tx.commit().context("commit lesson approval initialization")
}

pub(super) fn put(db: &Connection, event: &NewEvent) -> Result<()> {
    if let Some((sha, row)) = attribution(event) {
        db.execute(
            "INSERT OR IGNORE INTO lesson_approvals(sha, task, run) VALUES (?1, ?2, ?3)",
            params![sha, row.task, json(&row.run)?],
        )
        .context("index lesson approval")?;
    }
    Ok(())
}

pub(super) fn read(db: &Connection, sha: &str) -> Result<Vec<LessonApproval>> {
    load::all(
        db,
        "SELECT task, run FROM lesson_approvals WHERE sha = ?1 ORDER BY task, run",
        [sha],
        |r| {
            Ok(LessonApproval {
                task: load::text(r, 0)?,
                run: parse_json(&load::text(r, 1)?)?,
            })
        },
    )
}
