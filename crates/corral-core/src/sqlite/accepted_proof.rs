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
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'accepted_proof_metadata' AND value = '2')",
            [],
            |r| r.get(0),
        )
        .context("check accepted proof metadata version")?;
    if exists {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('accepted_proof_metadata', '2') ON CONFLICT(key) DO UPDATE SET value = '2'",
        [],
    )
    .context("record accepted proof metadata version")?;
    let current = load::all(
        &tx,
        "SELECT task, json_extract(doc, '$.id') FROM task_rows WHERE part = 'run' AND current = 1",
        [],
        |r| Ok((load::num(r, 0)?, load::text(r, 1)?)),
    )?
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();
    let mut ordinals = std::collections::BTreeMap::<(u64, String), u64>::new();
    let mut proofs = std::collections::BTreeMap::<u64, AcceptedProof>::new();
    for (task, key, mut doc, idx) in load::all(
        &tx,
        "SELECT task, key, doc, idx FROM task_rows WHERE part = 'artifact' ORDER BY task, key",
        [],
        |r| {
            Ok((
                load::num(r, 0)?,
                load::text(r, 1)?,
                load::doc(r, 2)?,
                load::text(r, 3)?,
            ))
        },
    )? {
        if work::in_hot_scope() {
            work::decoded(
                Counter::HistoryRowDecodes,
                u64::try_from(serde_json::to_vec(&doc)?.len())?,
                Subject::Plain,
            );
        }
        if doc.get("path").is_none() {
            continue;
        }
        let artifact: crate::Artifact = serde_json::from_value(doc.clone())?;
        let ordinal = ordinals.entry((task, artifact.run.clone())).or_default();
        *ordinal += 1;
        doc["_run_ordinal"] = format!("{ordinal:020}").into();
        let mut tokens: Vec<String> = serde_json::from_str(&idx)?;
        if artifact.purged_at.is_some() {
            let token = format!("run-purged:{}:{}", artifact.run.len(), artifact.run);
            tx.execute(
                "INSERT OR IGNORE INTO task_row_index(task, part, token, key) VALUES (?1,'artifact',?2,?3)",
                params![super::codec::int(task)?,token,key])?;
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
        tx.execute(
            "UPDATE task_rows SET doc=?3,idx=?4 WHERE task=?1 AND part='artifact' AND key=?2",
            params![
                super::codec::int(task)?,
                key,
                serde_json::to_string(&doc)?,
                serde_json::to_string(&tokens)?
            ],
        )?;
        if current.get(&task).is_some_and(|run| *run == artifact.run) {
            proofs.entry(task).or_default().fold(&artifact);
        }
    }
    for task in load::all(&tx, "SELECT num FROM tasks", [], |r| load::num(r, 0))? {
        let proof = proofs.remove(&task).unwrap_or_default();
        tx.execute(
            "UPDATE tasks SET history=json_set(history,'$.current_artifacts.accepted_proof',json(?2)) WHERE num=?1",
            params![super::codec::int(task)?,serde_json::to_string(&proof)?])?;
    }
    for ((task, run), count) in ordinals {
        let task_id = format!("T-{task:03}");
        tx.execute(
            "INSERT INTO records(area,key,version,doc,updated) VALUES (?1,?2,1,?3,?4)
             ON CONFLICT(area,key) DO UPDATE SET version=version+1,doc=excluded.doc",
            params![
                crate::backend::artifact_counts::AREA,
                format!("{task_id}/{run}"),
                serde_json::to_string(&format!("{count:020}"))?,
                super::codec::ts(jiff::Timestamp::UNIX_EPOCH)
            ],
        )?;
    }
    // Old chain receipts, including empty-run observations, cannot authorize the new facts.
    tx.execute("DELETE FROM records WHERE area=?1", [INTEGRITY_CHECKS])
        .context("retire pre-accumulator integrity observations")?;
    tx.commit().context("commit accepted proof initialization")
}

impl Tx<'_> {
    pub(super) fn maintain_artifact_count(
        &mut self,
        task: TaskNum,
        part: TaskPart,
        key: &str,
        after: Option<&HistoryRow>,
    ) -> Applied<()> {
        if part != TaskPart::Artifact {
            return Ok(());
        }
        let before = self.row(task, part, key)?;
        for (run, delta) in crate::backend::artifact_counts::changes(
            before.as_ref().map(|r| &r.doc),
            after.map(|r| &r.doc),
        ) {
            let key = format!("{}/{run}", self.task_id(task)?);
            let stored = load::first(
                self.db,
                "SELECT version,doc FROM records WHERE area=?1 AND key=?2",
                params![crate::backend::artifact_counts::AREA, key],
                |r| Ok((load::num(r, 0)?, load::doc(r, 1)?)),
            )
            .or_io()?;
            let doc =
                crate::backend::artifact_counts::adjusted(stored.as_ref().map(|(_, d)| d), delta)
                    .or_io()?;
            self.put_record(&RecordWrite {
                area: crate::backend::artifact_counts::AREA.into(),
                key,
                expected: stored.map(|(version, _)| version),
                doc: Some(doc),
                updated: self.now,
            })?;
        }
        Ok(())
    }

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
