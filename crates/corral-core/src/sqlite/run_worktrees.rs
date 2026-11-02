//! Output-sized scalar projection of coding-run locations. The covering expression index
//! is maintained by SQLite with every row write (including imports, deletes and head edits).
//! No task or run JSON is read on the hot path; old runs cannot inflate current-only reads.
use anyhow::Result;
use rusqlite::Connection;

use super::load;
use crate::backend::{RunWorktree, TaskNum};

pub(super) const INDEX: &str = "
CREATE INDEX IF NOT EXISTS task_rows_worktrees ON task_rows(
    current, task, key, json_extract(doc, '$.id'), json_extract(doc, '$.worktree')
) WHERE part = 'run';";

pub(super) const ALL: &str =
    "SELECT task, json_extract(doc, '$.id'), json_extract(doc, '$.worktree')
    FROM task_rows WHERE part = 'run' ORDER BY current, task, key";
pub(super) const CURRENT: &str =
    "SELECT task, json_extract(doc, '$.id'), json_extract(doc, '$.worktree')
    FROM task_rows WHERE part = 'run' AND current = 1 ORDER BY current, task, key";

pub(super) fn read(db: &Connection, current_only: bool) -> Result<Vec<RunWorktree>> {
    Ok(
        load::all(db, if current_only { CURRENT } else { ALL }, [], |row| {
            // Partial raw backend fixtures may have no location; canonical runs always do.
            let task = TaskNum(load::num(row, 0)?);
            let run = load::opt_text(row, 1)?;
            let path = load::opt_text(row, 2)?;
            Ok(run.zip(path).map(|(run, worktree)| RunWorktree {
                task,
                run,
                worktree: worktree.into(),
            }))
        })?
        .into_iter()
        .flatten()
        .collect(),
    )
}

#[cfg(test)]
#[path = "run_worktrees_tests.rs"]
mod tests;
