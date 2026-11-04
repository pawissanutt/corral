//! One-time native retention metadata backfill, before any read view is opened.
//! Imports and full writes derive the same metadata in the canonical splitter.
#[cfg(test)]
#[path = "native_retention_tests.rs"]
mod tests;
use crate::agent::NativeRecord;
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::Value;

pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin native retention initialization")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key='native_retention_index')",
            [],
            |r| r.get(0),
        )
        .context("check native retention version")?;
    if exists {
        return Ok(());
    }
    let rows = super::load::all(
        &tx,
        "SELECT task,part,key,doc FROM task_rows
         WHERE part IN ('run','review_run','plan_run','plan_review')
            OR (part='agent_record' AND (key GLOB '*/turns/*' OR key GLOB '*/receipts/*'))
         ORDER BY task,part,key",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    )?;
    for (task, part, key, raw) in rows {
        let mut doc: Value = serde_json::from_str(&raw)?;
        if let Some(records) = doc
            .get_mut("agent_host")
            .and_then(|host| host.get_mut("_records"))
        {
            records["live"] = records["counts"].clone();
            tx.execute(
                "UPDATE task_rows SET doc=?4 WHERE task=?1 AND part=?2 AND key=?3",
                params![task, part, key, serde_json::to_string(&doc)?],
            )
            .context("backfill native live counts")?;
        }
        if part != "agent_record" {
            continue;
        }
        let Some((owner, tail)) = key.rsplit_once('/') else {
            continue;
        };
        let Some((owner, kind)) = owner.rsplit_once('/') else {
            continue;
        };
        if !matches!(kind, "turns" | "receipts") {
            continue;
        }
        let record = NativeRecord::from_doc(kind, doc)?;
        let (_, tokens) = record.metadata();
        tx.execute(
            "UPDATE task_rows SET idx=?3 WHERE task=?1 AND part='agent_record' AND key=?2",
            params![task, key, serde_json::to_string(&tokens)?],
        )
        .context("backfill native retention tokens")?;
        for token in tokens {
            tx.execute("INSERT OR IGNORE INTO task_row_index(task,part,token,key) VALUES (?1,'agent_record',?2,?3)",
                params![task, token, key]).context("index native retention token")?;
        }
        if let NativeRecord::Turn(turn) = record {
            let order = format!(
                "{owner}/turn_order/{:010}/{:020}",
                turn.generation, turn.seq
            );
            let canonical = format!("{owner}/turns/{tail}");
            tx.execute("INSERT OR REPLACE INTO task_rows(task,part,key,current,open,doc,idx) VALUES (?1,'agent_record',?2,0,0,?3,'[]')",
                params![task, order, serde_json::to_string(&serde_json::json!({"key": canonical}))?])
                .context("backfill sequence-ordered turns")?;
        }
    }
    tx.execute(
        "INSERT INTO meta(key,value) VALUES ('native_retention_index','1')",
        [],
    )
    .context("record native retention version")?;
    tx.commit()
        .context("commit native retention initialization")
}
