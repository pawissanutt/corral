//! The SQLite implementation of the storage contract (plan T-253 sections 1-2 and 11), and the
//! only place in corral that names SQL or a database mechanism.
//!
//! The database is `.corral/db/corral.db` in WAL mode with `synchronous = FULL` and foreign keys
//! on. Every `apply` is one `BEGIN IMMEDIATE` transaction holding the operation's rows, events,
//! blob references, aggregates and change entry; every precondition is a logical check inside
//! that transaction (revision, version or `from` state), so correctness never depends on SQLite
//! having a single writer. Each view is one deferred read transaction on a pooled connection,
//! pinned when it opens; a WAL reader blocks no writer, and dropping the view releases its
//! snapshot so a checkpoint can complete.

mod accepted_proof;
mod artifact_reviews;
mod check;
mod codec;
mod fixture;
pub mod fs;
pub mod guard;
mod lesson_approvals;
mod lesson_artifacts;
mod load;
pub mod migrate;
mod plans;
mod row_indexes;
mod schema;
mod tx;
mod tx_blobs;
mod tx_storage;
mod tx_tasks;
mod view;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, TransactionBehavior, params};

pub use artifact_reviews::TABLES as ARTIFACT_REVIEW_TABLES;
pub use fs::{FilesystemProbe, is_refused_filesystem, statfs_probe};
pub use lesson_approvals::TABLES as LESSON_APPROVAL_TABLES;
pub use lesson_artifacts::TABLES as LESSON_ARTIFACT_TABLES;
pub use schema::{BACKEND_TABLES, SCHEMA_VERSION, SECTION_11};

use crate::backend::*;
use crate::blob_ref::BlobHash;

pub const DB_DIR: &str = "db";
pub const DB_FILE: &str = "corral.db";
/// How long a writer waits for another writer's transaction before `ApplyError::Busy`.
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// A point inside `apply`: after each primitive write (`what` names it), `commit` just before
/// the transaction commits, and `committed` just after. Crash tests stop a process here.
#[derive(Debug, Clone, Copy)]
pub struct BarrierPoint<'a> {
    pub op: OpKind,
    pub what: &'a str,
}

pub type Barrier = Arc<dyn Fn(BarrierPoint<'_>) + Send + Sync>;

#[derive(Clone)]
pub struct SqliteOptions {
    pub config: BackendConfig,
    pub clock: Arc<dyn Clock>,
    pub faults: Arc<FaultInjector>,
    pub filesystem: FilesystemProbe,
    pub barrier: Option<Barrier>,
}

impl Default for SqliteOptions {
    fn default() -> Self {
        Self {
            config: BackendConfig::default(),
            clock: Arc::new(SystemClock),
            faults: Arc::new(FaultInjector::default()),
            filesystem: statfs_probe,
            barrier: None,
        }
    }
}

pub(crate) struct Hooks {
    faults: Arc<FaultInjector>,
    barrier: Option<Barrier>,
    op: OpKind,
}

impl Hooks {
    fn hit(&self, what: &str) -> Result<(), ApplyError> {
        self.faults.after_write(what)?;
        self.point(what);
        Ok(())
    }

    fn point(&self, what: &str) {
        if let Some(barrier) = &self.barrier {
            barrier(BarrierPoint { op: self.op, what });
        }
    }
}

/// The connection settings and schema version a handle observes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pragmas {
    pub journal_mode: String,
    pub synchronous: i64,
    pub foreign_keys: bool,
    pub schema_version: u32,
}

/// The result of a `TRUNCATE` checkpoint: `busy` when a reader or writer kept it from
/// completing, else every WAL frame was copied back and the WAL file truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpoint {
    pub busy: bool,
    pub wal_frames: i64,
    pub checkpointed: i64,
}

/// One handle on a database: a writer connection and a pool of read connections. Handles on
/// the same file (in one process or many) coordinate only through SQLite's file locks.
pub struct SqliteBackend {
    path: PathBuf,
    options: SqliteOptions,
    writer: Mutex<Connection>,
    readers: Arc<Mutex<Vec<Connection>>>,
    /// Unfinished JSONL fixture input, never consulted by database readers.
    fixture_event_input: Mutex<String>,
    /// Kept alive by every view this handle opens (the store gate of its opener).
    retain: Option<Keepalive>,
}

/// Anything a handle's views keep alive until they close.
pub type Keepalive = Arc<dyn std::any::Any + Send + Sync>;

thread_local! {
    static BUSY_WAITS: Cell<u64> = const { Cell::new(0) };
}
const BUSY_DELAYS_MS: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];

fn busy_delay(attempt: usize) -> u64 {
    BUSY_DELAYS_MS.get(attempt).copied().unwrap_or(100)
}

fn busy_wait(count: i32) -> bool {
    BUSY_WAITS.with(|waits| waits.set(waits.get() + 1));
    let attempt = usize::try_from(count).unwrap_or(0);
    let waited: u64 = (0..attempt).map(busy_delay).sum();
    let delay = busy_delay(attempt);
    if u128::from(waited + delay) > BUSY_TIMEOUT.as_millis() {
        return false;
    }
    std::thread::sleep(Duration::from_millis(delay));
    true
}

fn connect(path: &Path) -> Result<Connection> {
    let conn =
        Connection::open(path).with_context(|| format!("open database {}", path.display()))?;
    conn.busy_handler(Some(busy_wait))
        .context("install busy handler")?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
        .context("configure connection")?;
    Ok(conn)
}

/// Switches to WAL. SQLite reports a journal-mode change that races another opener as busy
/// without consulting the busy handler, so the change is retried on the busy schedule here.
fn enable_wal(conn: &Connection) -> Result<()> {
    let mut attempt = 0;
    let mode: String = loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)) {
            Ok(mode) => break mode,
            Err(error) if codec::is_busy(&error) && busy_wait(attempt) => attempt += 1,
            Err(error) => return Err(error).context("enable WAL"),
        }
    };
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(anyhow!("the database stayed in journal mode {mode}"));
    }
    Ok(())
}

impl SqliteBackend {
    pub fn db_path(corral_dir: &Path) -> PathBuf {
        corral_dir.join(DB_DIR).join(DB_FILE)
    }

    /// Opens (creating when absent) `<corral_dir>/db/corral.db`.
    pub fn open(corral_dir: &Path, options: SqliteOptions) -> Result<Self> {
        Self::open_file(&Self::db_path(corral_dir), options)
    }

    /// Opens (creating when absent) the database at `path`. A path on a refused filesystem is
    /// refused before any file or directory is created.
    pub fn open_file(path: &Path, options: SqliteOptions) -> Result<Self> {
        fs::ensure_local(path, options.filesystem)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let mut writer = connect(path)?;
        enable_wal(&writer)?;
        let seed = format!(
            "{}:{}:{}",
            path.display(),
            std::process::id(),
            options.clock.now()
        );
        let workspace_id = BlobHash::of(seed.as_bytes()).as_str()[..32].to_owned();
        schema::init(&mut writer, &workspace_id)?;
        artifact_reviews::init(&mut writer)?;
        accepted_proof::init(&mut writer)?;
        lesson_artifacts::init(&mut writer)?;
        lesson_approvals::init(&mut writer)?;
        Ok(Self {
            path: path.to_owned(),
            options,
            writer: Mutex::new(writer),
            readers: Arc::new(Mutex::new(Vec::new())),
            fixture_event_input: Mutex::new(String::new()),
            retain: None,
        })
    }

    /// Makes every view this handle opens keep `keep` alive until the view closes, so a view
    /// that outlives its opener still holds what the opener held.
    pub fn retain_in_views(&mut self, keep: Keepalive) {
        self.retain = Some(keep);
    }

    /// Another handle on the same database with its own connections.
    pub fn handle(&self) -> Result<Self> {
        Self::open_file(&self.path, self.options.clone())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Busy-handler invocations on this thread: how often its writes waited on a lock.
    pub fn busy_waits() -> u64 {
        BUSY_WAITS.with(Cell::get)
    }

    fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn reader(&self) -> Result<Connection> {
        let pooled = self
            .readers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        match pooled {
            Some(conn) => Ok(conn),
            None => {
                let conn = connect(&self.path)?;
                conn.execute_batch("PRAGMA query_only = ON")
                    .context("make the read connection query-only")?;
                Ok(conn)
            }
        }
    }

    fn give_back(&self, conn: Connection) {
        self.readers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(conn);
    }

    pub fn pragmas(&self) -> Result<Pragmas> {
        let conn = self.reader()?;
        let read = || -> Result<Pragmas> {
            let journal_mode: String =
                conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
            let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
            let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
            let version = load::meta_u64(&conn, "schema_version")?;
            Ok(Pragmas {
                journal_mode,
                synchronous,
                foreign_keys: foreign_keys == 1,
                schema_version: u32::try_from(version)?,
            })
        };
        let pragmas = read();
        self.give_back(conn);
        pragmas
    }

    /// One `meta` value of this database (`origin`, `workspace_id`, `migration_digest`).
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.reader()?;
        let value = load::first(
            &conn,
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| load::text(row, 0),
        );
        self.give_back(conn);
        value
    }

    /// Removes a task and every row of it as a lost or foreign deletion would, recording one
    /// change entry so readers notice (test fixtures only).
    #[doc(hidden)]
    pub fn fixture_delete_task(&self, num: TaskNum) -> Result<()> {
        self.fixture_change(num, |tx, task| {
            let (status, project): (String, Option<String>) = load::first(
                tx,
                "SELECT status, project FROM tasks WHERE num = ?1",
                params![task],
                |row| Ok((load::text(row, 0)?, load::opt_text(row, 1)?)),
            )?
            .with_context(|| format!("task {} does not exist", num.0))?;
            for table in [
                "task_row_index",
                "task_rows",
                "task_edges",
                "artifact_keep_paths",
                "artifact_review_counts",
            ] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE task = ?1"),
                    params![task],
                )?;
            }
            tx.execute("DELETE FROM tasks WHERE num = ?1", params![task])?;
            tx.execute(
                "UPDATE status_counts SET n = n - 1 WHERE status = ?1 AND project = ?2",
                params![status, project.unwrap_or_default()],
            )?;
            tx.execute("DELETE FROM status_counts WHERE n <= 0", [])?;
            Ok(())
        })
    }

    /// Replaces a task's head state document with `state` verbatim, as on-disk corruption
    /// would, recording one change entry (test fixtures only).
    #[doc(hidden)]
    pub fn fixture_raw_task_state(&self, num: TaskNum, state: &str) -> Result<()> {
        self.fixture_change(num, |tx, task| {
            let changed = tx.execute(
                "UPDATE tasks SET state = ?2 WHERE num = ?1",
                params![task, state],
            )?;
            anyhow::ensure!(changed == 1, "task {} does not exist", num.0);
            Ok(())
        })
    }

    fn fixture_change(
        &self,
        num: TaskNum,
        change: impl FnOnce(&rusqlite::Transaction<'_>, i64) -> Result<()>,
    ) -> Result<()> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        change(&tx, codec::int(num.0)?)?;
        let seq = load::meta_u64(&tx, "change_seq")? + 1;
        tx.execute(
            "INSERT INTO changes(seq, at, op) VALUES (?1, ?2, ?3)",
            params![
                codec::int(seq)?,
                codec::ts(self.options.clock.now()),
                codec::name(&OpKind::UpdateTask)?
            ],
        )?;
        let scope = ChangeScope::Task {
            num,
            parts: Vec::new(),
            history: false,
            demoted: false,
        };
        tx.execute(
            "INSERT INTO change_scopes(seq, kind, key) VALUES (?1, 'task', ?2)",
            params![codec::int(seq)?, codec::json(&scope)?],
        )?;
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'change_seq'",
            params![seq.to_string()],
        )?;
        tx.commit().context("commit a fixture change")
    }

    /// Drops change entries up to and including `through`, as retention does; later reads
    /// before the floor fail with `FeedError::Gap`.
    pub fn prune_changes_through(&self, through: ChangeSeq) -> Result<()> {
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq = codec::int(through.0)?;
        tx.execute("DELETE FROM change_scopes WHERE seq <= ?1", params![seq])?;
        tx.execute("DELETE FROM changes WHERE seq <= ?1", params![seq])?;
        let floor = load::meta_u64(&tx, "feed_floor")?.max(through.0);
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'feed_floor'",
            params![floor.to_string()],
        )?;
        tx.commit().context("commit change-feed pruning")
    }

    /// Runs a `TRUNCATE` checkpoint on this handle's writer connection.
    pub fn checkpoint_truncate(&self) -> Result<WalCheckpoint> {
        let conn = self.writer();
        let (busy, wal_frames, checkpointed): (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .context("wal_checkpoint(TRUNCATE)")?;
        Ok(WalCheckpoint {
            busy: busy != 0,
            wal_frames,
            checkpointed,
        })
    }
}

impl StoreBackend for SqliteBackend {
    fn view(&self) -> Result<Box<dyn ReadView + Send>, ViewError> {
        self.view_with_max_age(self.options.config.view_max_age)
    }

    fn view_with_max_age(&self, max_age: Duration) -> Result<Box<dyn ReadView + Send>, ViewError> {
        let conn = self.reader().map_err(ViewError::Io)?;
        let view = view::SqliteView::open(
            conn,
            Arc::clone(&self.readers),
            Arc::clone(&self.options.clock),
            max_age,
            self.retain.clone(),
        )?;
        Ok(Box::new(view))
    }

    fn apply(&self, op: Operation) -> Result<Committed, ApplyError> {
        let hooks = Hooks {
            faults: Arc::clone(&self.options.faults),
            barrier: self.options.barrier.clone(),
            op: op.kind(),
        };
        let mut conn = self.writer();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let in_write = guard::WriteScope::enter();
        let now = self.options.clock.now();
        let committed = tx::run(&tx, op, now, &hooks)?;
        hooks.point("commit");
        tx.commit()?;
        drop(in_write);
        hooks.point("committed");
        Ok(committed)
    }

    fn check(&self) -> Result<IntegrityReport> {
        let conn = self.reader()?;
        conn.execute_batch("BEGIN DEFERRED")?;
        let report = check::check(&conn);
        if conn.execute_batch("ROLLBACK").is_ok() {
            self.give_back(conn);
        }
        report
    }

    /// Copies the database only, as one consistent snapshot taken while writers continue; blob
    /// content is not included (plan section 7, backup). `dest` must not exist.
    fn backup_database(&self, dest: &Path) -> Result<()> {
        let conn = connect(&self.path)?;
        let dest_text = dest
            .to_str()
            .ok_or_else(|| anyhow!("backup path {} is not UTF-8", dest.display()))?;
        conn.execute("VACUUM INTO ?1", params![dest_text])
            .with_context(|| format!("back up the database to {}", dest.display()))?;
        Ok(())
    }
}
