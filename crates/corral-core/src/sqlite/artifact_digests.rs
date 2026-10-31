//! One-time derivation of the first-registration lookup, before opening any read view.
//! The index keeps every registration: oldest row order, not wall time, selects the first.
use anyhow::{Context, Result};
use rusqlite::{Connection, TransactionBehavior, params};

use super::load;

pub(super) fn init(db: &mut Connection) -> Result<()> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin artifact digest index initialization")?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key = 'artifact_digest_index')",
            [],
            |r| r.get(0),
        )
        .context("check artifact digest index version")?;
    if exists {
        return Ok(());
    }
    let rows = load::all(
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
    )?;
    for (task, key, doc, idx) in rows {
        // Raw partial backend fixtures, unlike canonical artifact registrations, have no identity.
        let (Some(run), Some(sha)) = (doc["run"].as_str(), doc["sha256"].as_str()) else {
            continue;
        };
        let token = crate::store::artifact_digest_token(run, sha);
        let mut tokens: Vec<String> =
            serde_json::from_str(&idx).context("decode artifact index tokens")?;
        if !tokens.contains(&token) {
            tokens.push(token.clone());
        }
        tx.execute(
            "INSERT OR IGNORE INTO task_row_index(task, part, token, key) VALUES (?1, 'artifact', ?2, ?3)",
            params![super::codec::int(task)?, token, key],
        ).context("derive artifact digest lookup")?;
        tx.execute(
            "UPDATE task_rows SET idx = ?3 WHERE task = ?1 AND part = 'artifact' AND key = ?2",
            params![
                super::codec::int(task)?,
                key,
                serde_json::to_string(&tokens)?
            ],
        )
        .context("retain artifact digest token for exact-key deletion")?;
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('artifact_digest_index', '1')",
        [],
    )
    .context("record artifact digest index version")?;
    tx.commit()
        .context("commit artifact digest index initialization")
}

#[cfg(test)]
#[path = "artifact_digests_tests.rs"]
mod tests;
