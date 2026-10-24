//! `StoreBackend::check`: SQLite's own integrity and foreign-key checks plus the derived
//! invariants the operations maintain (status counts, blob reference counts and states, open
//! condition pointers and the change sequence).

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::Connection;

use super::load::{self, all, num, opt_text, text};
use crate::backend::{BlobState, IntegrityReport};

pub(super) fn check(db: &Connection) -> Result<IntegrityReport> {
    let mut problems = Vec::new();
    let integrity = all(db, "PRAGMA integrity_check", [], |row| text(row, 0))?;
    if integrity != ["ok"] {
        problems.extend(
            integrity
                .into_iter()
                .map(|p| format!("integrity_check: {p}")),
        );
    }
    let foreign = all(db, "PRAGMA foreign_key_check", [], |row| {
        Ok(format!(
            "foreign key from {} to {} is broken",
            text(row, 0)?,
            text(row, 2)?
        ))
    })?;
    problems.extend(foreign);

    let derived = all(
        db,
        "SELECT status, COALESCE(project, ''), COUNT(*) FROM tasks GROUP BY 1, 2 ORDER BY 1, 2",
        [],
        |row| Ok((text(row, 0)?, text(row, 1)?, num(row, 2)?)),
    )?;
    let stored: Vec<(String, String, u64)> = load::status_counts(db)?
        .into_iter()
        .map(|c| (c.status, c.project, c.n))
        .collect();
    if derived != stored {
        problems.push(format!(
            "status counts {stored:?} differ from heads {derived:?}"
        ));
    }

    let refs: BTreeMap<String, u64> = all(
        db,
        "SELECT blob, COUNT(*) FROM blob_refs GROUP BY blob",
        [],
        |row| Ok((text(row, 0)?, num(row, 1)?)),
    )?
    .into_iter()
    .collect();
    let blobs = all(
        db,
        "SELECT hash, state, refs, unreferenced_since FROM blobs ORDER BY hash",
        [],
        |row| {
            Ok((
                text(row, 0)?,
                super::codec::from_name::<BlobState>(&text(row, 1)?)?,
                num(row, 2)?,
                opt_text(row, 3)?,
            ))
        },
    )?;
    for (hash, state, counted, since) in blobs {
        let found = refs.get(&hash).copied().unwrap_or(0);
        if counted != found {
            problems.push(format!("blob {hash} counts {counted} refs, found {found}"));
        }
        if found > 0 && state != BlobState::Live {
            problems.push(format!("referenced blob {hash} is {state:?}"));
        }
        if counted == 0 && state == BlobState::Live && since.is_none() {
            problems.push(format!(
                "unreferenced live blob {hash} lacks unreferenced_since"
            ));
        }
    }

    let dangling = all(
        db,
        "SELECT c.task, c.kind FROM conditions c LEFT JOIN condition_runs r \
         ON r.opened_seq = c.open_run WHERE r.opened_seq IS NULL OR r.closed_seq IS NOT NULL \
         OR r.task != c.task OR r.kind != c.kind",
        [],
        |row| Ok((text(row, 0)?, text(row, 1)?)),
    )?;
    for (task, kind) in dangling {
        problems.push(format!(
            "condition {task}/{kind} points at a closed or missing run"
        ));
    }

    let change_seq = load::meta_u64(db, "change_seq")?;
    let newest = all(db, "SELECT COALESCE(MAX(seq), 0) FROM changes", [], |row| {
        num(row, 0)
    })?;
    if newest.first().copied().unwrap_or(0) > change_seq {
        problems.push(format!(
            "change entries run past meta.change_seq {change_seq}"
        ));
    }
    Ok(IntegrityReport { problems })
}
