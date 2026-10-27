//! One-time backfill of accepted-proof facts, at database open before hot evaluations.
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior, params};

use super::codec::OrIo;
use super::load;
use super::tx::{Applied, Tx};
use crate::accepted_proof::AcceptedProof;
use crate::accepted_proof::{INTEGRITY_CHECKS, changed_runs};
use crate::backend::{HistoryRow, RecordWrite, TaskNum, TaskPart};
use crate::work::{self, Counter, Subject};

pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin accepted proof initialization")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'accepted_proof_metadata')",
            [],
            |r| r.get(0),
        )
        .context("check accepted proof metadata version")?;
    if exists {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('accepted_proof_metadata', '1')",
        [],
    )
    .context("record accepted proof metadata version")?;
    let current = load::all(
        &tx,
        "SELECT task, json_extract(doc, '$.id') FROM task_rows
         WHERE part = 'run' AND current = 1",
        [],
        |r| Ok((load::num(r, 0)?, load::text(r, 1)?)),
    )?;
    for (task, run) in current {
        let mut proof = AcceptedProof::default();
        for doc in load::all(
            &tx,
            "SELECT doc FROM task_rows
             WHERE task = ?1 AND part = 'artifact' AND json_extract(doc, '$.run') = ?2
             ORDER BY key",
            params![super::codec::int(task)?, run],
            |r| load::doc(r, 0),
        )? {
            // Whole-call budgets must see this one-time preparation if they surround open.
            // Normal evaluations return at the persisted version check above.
            if work::in_hot_scope() {
                work::decoded(
                    Counter::HistoryRowDecodes,
                    u64::try_from(serde_json::to_vec(&doc)?.len())?,
                    Subject::Plain,
                );
            }
            // The low-level blob promotion fixture is not an Artifact registration.
            if doc.get("path").is_some() {
                proof.fold(&serde_json::from_value(doc)?);
            }
        }
        tx.execute(
            "UPDATE tasks SET history = json_set(history,
             '$.current_artifacts.accepted_proof', json(?2)) WHERE num = ?1",
            params![super::codec::int(task)?, serde_json::to_string(&proof)?],
        )
        .context("backfill current run accepted proof")?;
    }
    tx.commit().context("commit accepted proof initialization")
}

impl Tx<'_> {
    pub(super) fn invalidate_accepted_proof(
        &mut self,
        task: TaskNum,
        part: TaskPart,
        key: &str,
        after: Option<&HistoryRow>,
    ) -> Applied<()> {
        let before = self.row(task, part, key)?;
        for run in changed_runs(before.as_ref(), after) {
            let key = format!("{}/{run}", self.task_id(task)?);
            let version = load::first(
                self.db,
                "SELECT version FROM records WHERE area = ?1 AND key = ?2",
                params![INTEGRITY_CHECKS, key],
                |r| load::num(r, 0),
            )
            .or_io()?;
            if let Some(version) = version {
                self.put_record(&RecordWrite {
                    area: INTEGRITY_CHECKS.into(),
                    key,
                    expected: Some(version),
                    doc: None,
                    updated: self.now,
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "accepted_proof_tests.rs"]
mod tests;
