//! Durable, keyed reviewed-Keep counts. Maintenance shares the artifact/record transaction.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

use super::{codec::int, load};
use crate::backend::{TaskNum, artifact_reviews::kept_path};

/// Initializes old databases once, at database open rather than in a hot evaluation.
pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin artifact facts initialization")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'artifact_reviewed_paths')",
            [],
            |r| r.get(0),
        )
        .context("check artifact facts schema")?;
    if exists {
        return Ok(());
    }
    tx.execute_batch(
        "CREATE TABLE artifact_reviewed_paths(path TEXT PRIMARY KEY) STRICT;
         CREATE TABLE artifact_keep_paths(task INTEGER NOT NULL, key TEXT NOT NULL,
             path TEXT NOT NULL, PRIMARY KEY(task, key)) STRICT;
         CREATE INDEX artifact_keep_paths_path ON artifact_keep_paths(path, task, key);
         CREATE TABLE artifact_review_counts(task INTEGER PRIMARY KEY, n INTEGER NOT NULL) STRICT;",
    )
    .context("create artifact facts tables")?;
    let reviewed: Option<Value> = load::first(
        &tx,
        "SELECT doc FROM records WHERE area = 'reviewed-artifacts' AND key = 'set'",
        [],
        |r| load::doc(r, 0),
    )?;
    replace(&tx, reviewed.as_ref())?;
    let rows = load::all(
        &tx,
        "SELECT task, key, doc, idx FROM task_rows WHERE part = 'artifact' ORDER BY task, key",
        [],
        |r| {
            Ok((
                TaskNum(load::num(r, 0)?),
                load::text(r, 1)?,
                load::doc(r, 2)?,
                load::text(r, 3)?,
            ))
        },
    )?;
    let mut facts = BTreeMap::new();
    let runs = load::all(
        &tx,
        "SELECT task, json_extract(doc, '$.id') FROM task_rows WHERE part = 'run' AND current = 1",
        [],
        |r| Ok((TaskNum(load::num(r, 0)?), load::text(r, 1)?)),
    )?
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    for (task, key, doc, idx) in rows {
        put(&tx, task, &key, Some(&doc))?;
        if doc.get("path").is_none() {
            continue;
        }
        let artifact: crate::Artifact = serde_json::from_value(doc)?;
        if runs.get(&task).is_some_and(|run| *run == artifact.run) {
            facts
                .entry(task)
                .or_insert_with(crate::container_facts::ArtifactFacts::default)
                .fold(&artifact);
        }
        let mut tokens: Vec<String> = serde_json::from_str(&idx)?;
        for token in [
            format!("run-path:{}:{}", artifact.run, artifact.path),
            format!("run-title:{}:{}", artifact.run, artifact.title),
            format!("run-kind:{}:{}", artifact.run, artifact.kind.as_str()),
        ] {
            tx.execute(
                "INSERT OR IGNORE INTO task_row_index(task, part, token, key)
                VALUES (?1, 'artifact', ?2, ?3)",
                params![int(task.0)?, token, key],
            )?;
            if !tokens.contains(&token) {
                tokens.push(token);
            }
        }
        tx.execute(
            "UPDATE task_rows SET idx = ?3 WHERE task = ?1 AND part = 'artifact' AND key = ?2",
            params![int(task.0)?, key, serde_json::to_string(&tokens)?],
        )?;
    }
    for (task, mut summary) in load::all(&tx, "SELECT num, history FROM tasks", [], |r| {
        Ok((TaskNum(load::num(r, 0)?), load::doc(r, 1)?))
    })? {
        summary["current_artifacts"] =
            serde_json::to_value(facts.remove(&task).unwrap_or_default())?;
        tx.execute(
            "UPDATE tasks SET history = ?2 WHERE num = ?1",
            params![int(task.0)?, serde_json::to_string(&summary)?],
        )?;
    }
    tx.commit().context("commit artifact facts initialization")
}

fn adjust(db: &Connection, task: TaskNum, delta: i64) -> Result<()> {
    db.execute(
        "INSERT INTO artifact_review_counts(task, n) VALUES (?1, ?2)
         ON CONFLICT(task) DO UPDATE SET n = n + excluded.n",
        params![int(task.0)?, delta],
    )
    .context("adjust reviewed artifact count")?;
    Ok(())
}

fn reviewed(db: &Connection, path: &str) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM artifact_reviewed_paths WHERE path = ?1)",
        [path],
        |r| r.get(0),
    )
    .context("look up reviewed path")
}

pub(super) fn put(db: &Connection, task: TaskNum, key: &str, doc: Option<&Value>) -> Result<()> {
    let old: Option<String> = db
        .query_row(
            "SELECT path FROM artifact_keep_paths WHERE task = ?1 AND key = ?2",
            params![int(task.0)?, key],
            |r| r.get(0),
        )
        .optional()
        .context("look up Keep registration")?;
    let new = doc.map(kept_path).transpose()?.flatten();
    if old == new {
        return Ok(());
    }
    if let Some(path) = old {
        if reviewed(db, &path)? {
            adjust(db, task, -1)?;
        }
        db.execute(
            "DELETE FROM artifact_keep_paths WHERE task = ?1 AND key = ?2",
            params![int(task.0)?, key],
        )
        .context("remove Keep registration")?;
    }
    if let Some(path) = new {
        db.execute(
            "INSERT INTO artifact_keep_paths(task, key, path) VALUES (?1, ?2, ?3)",
            params![int(task.0)?, key, path],
        )
        .context("index Keep registration")?;
        if reviewed(db, &path)? {
            adjust(db, task, 1)?;
        }
    }
    Ok(())
}

pub(super) fn replace(db: &Connection, doc: Option<&Value>) -> Result<()> {
    let paths: BTreeSet<String> = doc
        .map(|d| serde_json::from_value(d.clone()))
        .transpose()?
        .unwrap_or_default();
    let old = load::all(db, "SELECT path FROM artifact_reviewed_paths", [], |r| {
        load::text(r, 0)
    })?
    .into_iter()
    .collect::<BTreeSet<_>>();
    for path in old.symmetric_difference(&paths) {
        let add = paths.contains(path);
        for task in load::all(
            db,
            "SELECT task FROM artifact_keep_paths WHERE path = ?1",
            [path],
            |r| Ok(TaskNum(load::num(r, 0)?)),
        )? {
            adjust(db, task, if add { 1 } else { -1 })?;
        }
        if add {
            db.execute(
                "INSERT INTO artifact_reviewed_paths(path) VALUES (?1)",
                [path],
            )?;
        } else {
            db.execute(
                "DELETE FROM artifact_reviewed_paths WHERE path = ?1",
                [path],
            )?;
        }
    }
    Ok(())
}

pub(super) fn count(db: &Connection, task: TaskNum) -> Result<u64> {
    Ok(load::first(
        db,
        "SELECT n FROM artifact_review_counts WHERE task = ?1",
        [int(task.0)?],
        |r| load::num(r, 0),
    )?
    .unwrap_or(0))
}
