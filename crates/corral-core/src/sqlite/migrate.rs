//! Database operations of `corral migrate` that the storage contract does not carry: the
//! candidate's identity in `meta`, the rows `ImportBatch` has no field for (legacy files,
//! claims, authorizations), and maintenance reads of a database no v2 process has open.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use super::codec::{int, json, name, ts};
use super::{SqliteBackend, WalCheckpoint, load};
use crate::backend::{AuthorizationRow, ChangeScope, ClaimRow, OpKind};
use crate::blob_ref::{self, BlobHash};

/// One file of the legacy tree as the migration classified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyFileRow {
    pub path: String,
    pub kind: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub blob: Option<BlobHash>,
    pub disposition: String,
}

/// Rows the import writes outside `ImportBatch`, in one transaction with one change entry.
#[derive(Debug, Clone, Default)]
pub struct MigrationRows {
    pub claims: Vec<ClaimRow>,
    pub authorizations: Vec<AuthorizationRow>,
    pub legacy_files: Vec<LegacyFileRow>,
}

/// What maintenance reads of a database file without opening it as a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbMeta {
    pub change_seq: u64,
    pub origin: Option<String>,
    pub migration_digest: Option<String>,
}

impl SqliteBackend {
    /// Marks this database as a migration candidate (`meta.origin = migrate`) of the migration
    /// named by `digest`.
    pub fn mark_migration(&self, digest: &str) -> Result<()> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE meta SET value = 'migrate' WHERE key = 'origin'", [])?;
        tx.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('migration_digest', ?1)",
            params![digest],
        )?;
        tx.commit().context("recording the migration identity")
    }

    pub fn change_seq(&self) -> Result<u64> {
        let conn = self.reader()?;
        let seq = load::meta_u64(&conn, "change_seq");
        self.give_back(conn);
        seq
    }

    /// Writes the migration rows, keeping blob reference counts exact, as one change.
    pub fn import_migration_rows(&self, rows: &MigrationRows, at: jiff::Timestamp) -> Result<()> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut refs: BTreeMap<(&'static str, String), Vec<BlobHash>> = BTreeMap::new();
        for claim in &rows.claims {
            tx.execute(
                "INSERT INTO claims(name, owner, acquired, doc) VALUES (?1, ?2, ?3, ?4)",
                params![
                    claim.name,
                    claim.owner,
                    ts(claim.acquired),
                    json(&claim.doc)?
                ],
            )
            .with_context(|| format!("importing claim {}", claim.name))?;
            let found = blob_ref::scan(&claim.doc);
            refs.insert(
                ("claim", json(&[&claim.name])?),
                found.into_iter().collect(),
            );
        }
        for auth in &rows.authorizations {
            tx.execute(
                "INSERT INTO authorizations(id, task, disposition, doc) VALUES (?1, ?2, ?3, ?4)",
                params![
                    auth.id,
                    auth.task,
                    name(&auth.disposition)?,
                    json(&auth.doc)?
                ],
            )
            .with_context(|| format!("importing authorization {}", auth.id))?;
            let found = blob_ref::scan(&auth.doc);
            refs.insert(
                ("authorization", json(&[&auth.id])?),
                found.into_iter().collect(),
            );
        }
        for file in &rows.legacy_files {
            tx.execute(
                "INSERT INTO legacy_files(path, kind, bytes, sha256, blob, disposition) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    file.path,
                    file.kind,
                    int(file.bytes)?,
                    file.sha256,
                    file.blob.as_ref().map(BlobHash::as_str),
                    file.disposition
                ],
            )
            .with_context(|| format!("importing legacy file {}", file.path))?;
            if let Some(blob) = &file.blob {
                refs.insert(("legacy_file", json(&[&file.path])?), vec![blob.clone()]);
            }
        }
        for ((kind, key), blobs) in &refs {
            for blob in blobs {
                let updated = tx.execute(
                    "UPDATE blobs SET refs = refs + 1, state = 'live', unreferenced_since = NULL \
                     WHERE hash = ?1 AND state = 'live'",
                    params![blob.as_str()],
                )?;
                ensure!(updated == 1, "{kind} {key} references unknown blob {blob}");
                tx.execute(
                    "INSERT INTO blob_refs(owner_kind, owner_key, blob) VALUES (?1, ?2, ?3)",
                    params![kind, key, blob.as_str()],
                )?;
            }
        }
        let seq = load::meta_u64(&tx, "change_seq")? + 1;
        tx.execute(
            "INSERT INTO changes(seq, at, op) VALUES (?1, ?2, ?3)",
            params![int(seq)?, ts(at), name(&OpKind::ImportBatch)?],
        )?;
        for area in ["claims", "authorizations", "legacy_files"] {
            let scope = ChangeScope::Record {
                area: area.to_owned(),
                key: String::new(),
            };
            tx.execute(
                "INSERT INTO change_scopes(seq, kind, key) VALUES (?1, 'record', ?2)",
                params![int(seq)?, json(&scope)?],
            )?;
        }
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'change_seq'",
            params![seq.to_string()],
        )?;
        tx.commit().context("committing the migration rows")
    }

    /// Every imported legacy file row, ordered by path.
    pub fn legacy_file_rows(&self) -> Result<Vec<LegacyFileRow>> {
        let conn = self.reader()?;
        let rows = load::all(
            &conn,
            "SELECT path, kind, bytes, sha256, blob, disposition FROM legacy_files ORDER BY path",
            [],
            |row| {
                Ok(LegacyFileRow {
                    path: load::text(row, 0)?,
                    kind: load::text(row, 1)?,
                    bytes: load::num(row, 2)?,
                    sha256: load::opt_text(row, 3)?,
                    blob: load::opt_text(row, 4)?
                        .map(|hash| BlobHash::parse(&hash))
                        .transpose()?,
                    disposition: load::text(row, 5)?,
                })
            },
        );
        self.give_back(conn);
        rows
    }
}

fn open_read_only(db_file: &Path) -> Result<Connection> {
    ensure!(db_file.is_file(), "{} does not exist", db_file.display());
    Connection::open_with_flags(
        db_file,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {} read-only", db_file.display()))
}

/// The migration metadata of the database at `db_file`, read without writing to it.
pub fn read_meta(db_file: &Path) -> Result<DbMeta> {
    let conn = open_read_only(db_file)?;
    let value = |key: &str| -> Result<Option<String>> {
        conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .with_context(|| format!("reading meta.{key} of {}", db_file.display()))
    };
    let change_seq = value("change_seq")?
        .context("the database has no change_seq")?
        .parse()
        .context("parsing meta.change_seq")?;
    Ok(DbMeta {
        change_seq,
        origin: value("origin")?,
        migration_digest: value("migration_digest")?,
    })
}

/// `PRAGMA integrity_check` of the database at `db_file`: empty when it passes.
pub fn integrity_problems(db_file: &Path) -> Result<Vec<String>> {
    let conn = open_read_only(db_file)?;
    let found = load::all(&conn, "PRAGMA integrity_check", [], |row| {
        load::text(row, 0)
    })?;
    Ok(if found == ["ok"] { Vec::new() } else { found })
}

/// A `TRUNCATE` checkpoint of the database at `db_file` on a fresh connection that changes no
/// schema, then closes it.
pub fn checkpoint_file(db_file: &Path) -> Result<WalCheckpoint> {
    ensure!(db_file.is_file(), "{} does not exist", db_file.display());
    let conn = Connection::open_with_flags(db_file, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("opening {}", db_file.display()))?;
    let (busy, wal_frames, checkpointed): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .context("wal_checkpoint(TRUNCATE)")?;
    conn.close()
        .map_err(|(_, error)| error)
        .with_context(|| format!("closing {}", db_file.display()))?;
    Ok(WalCheckpoint {
        busy: busy != 0,
        wal_frames,
        checkpointed,
    })
}
