use std::{path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;

use crate::{
    error::refuse,
    model::{Check, CheckKind, Role, Run, Status, Task},
};

pub const SCHEMA_VERSION: u32 = 1;

/// A single SQLite connection. Operations and their events share `transaction`.
/// A Store is deliberately not Sync; independent callers open independent connections.
pub struct Store {
    pub connection: Connection,
}

#[derive(Debug, Default, Clone)]
pub struct TaskFilter {
    pub statuses: Vec<Status>,
    pub project: Option<String>,
    pub parent: Option<String>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection =
            Connection::open(path).with_context(|| format!("open store {}", path.display()))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .context("configure SQLite busy timeout")?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .context("read schema version")?;
        if version > SCHEMA_VERSION {
            return Err(refuse(
                format!(
                    "store schema {version} is newer than supported {SCHEMA_VERSION}; update corral"
                ),
                vec![],
            ));
        }
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .context("enable SQLite WAL")?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .context("enable foreign keys")?;
        if version == 0 {
            connection
                .execute_batch(SCHEMA)
                .context("initialize engine schema")?;
        }
        connection
            .execute_batch(crate::usage::SCHEMA)
            .context("initialize usage table")?;
        Ok(Self { connection })
    }

    /// Compare successive results from the same connection to observe commits
    /// made elsewhere, including writes which do not append an event.
    pub fn data_version(&self) -> Result<u64> {
        self.connection
            .pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
            .context("read SQLite data version")?
            .try_into()
            .context("SQLite data version must not be negative")
    }

    /// Pin related reads without taking the writer lock.
    pub fn read_snapshot<T>(&self, read: impl FnOnce() -> Result<T>) -> Result<T> {
        let transaction = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Deferred,
        )
        .context("begin engine read snapshot")?;
        let result = read()?;
        transaction.commit().context("end engine read snapshot")?;
        Ok(result)
    }

    /// The closure's writes (including events) commit together; an error rolls back.
    /// Nested transactions are rejected by SQLite rather than partially committed.
    pub fn transaction<T>(&self, operation: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let transaction = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )
        .context("begin engine transaction")?;
        let result = operation(self)?;
        transaction.commit().context("commit engine transaction")?;
        Ok(result)
    }

    pub fn set_first_id(&self, first_id: u32) -> Result<()> {
        let count: i64 = self
            .connection
            .query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))
            .context("count existing tasks")?;
        ensure!(count == 0, "first_id may only be set on an empty store");
        self.connection
            .execute(
                "UPDATE meta SET value=?1 WHERE key='next_task'",
                [first_id.to_string()],
            )
            .context("set first task id")?;
        Ok(())
    }

    /// Call inside the creating transaction so a failed creation consumes no ID.
    pub fn next_task_id(&self) -> Result<String> {
        let number: i64 = self.connection.query_row(
            "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='next_task' AND CAST(value AS INTEGER)<=4294967295 RETURNING CAST(value AS INTEGER)-1", [], |row| row.get(0)
        ).context("allocate next task id")?;
        Ok(format!("T-{number:03}"))
    }

    pub fn insert_task(&self, task: &Task) -> Result<()> {
        validate_task(task)?;
        self.connection
            .execute(
                "INSERT INTO tasks(id,status,project,document) VALUES(?1,?2,?3,?4)",
                params![task.id, task.status.as_str(), task.project, encode(task)?],
            )
            .context("insert task")?;
        Ok(())
    }

    pub fn update_task(&self, task: &Task) -> Result<()> {
        validate_task(task)?;
        let changed = self
            .connection
            .execute(
                "UPDATE tasks SET status=?2,project=?3,document=?4 WHERE id=?1",
                params![task.id, task.status.as_str(), task.project, encode(task)?],
            )
            .context("update task")?;
        ensure!(changed == 1, "task {} does not exist", task.id);
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> Result<Option<Task>> {
        self.optional_document("SELECT document FROM tasks WHERE id=?1", [id])
    }

    pub fn require_task(&self, id: &str) -> Result<Task> {
        self.get_task(id)?.ok_or_else(|| {
            refuse(
                format!("task {id} does not exist; use corral list --all to find its ID"),
                vec![],
            )
        })
    }

    pub fn delete_task(&self, id: &str) -> Result<bool> {
        Ok(self
            .connection
            .execute("DELETE FROM tasks WHERE id=?1", [id])
            .context("delete task")?
            != 0)
    }

    /// Ordered by priority and creation time, then ID, suitable for dispatch.
    pub fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        let statuses = encode(&filter.statuses)?;
        let mut statement = self.connection.prepare("SELECT document FROM tasks WHERE (?1 IS NULL OR project=?1) AND (?2='[]' OR status IN (SELECT value FROM json_each(?2))) AND (?3 IS NULL OR json_extract(document,'$.parent')=?3) ORDER BY json_extract(document,'$.priority'),rtrim(json_extract(document,'$.created'),'Z'),CAST(substr(id,3) AS INTEGER),id").context("prepare task listing")?;
        documents(
            &mut statement,
            params![filter.project, statuses, filter.parent],
        )
    }

    /// Count open tasks by assigned project without reading task bodies.
    pub fn open_project_counts(&self) -> Result<std::collections::BTreeMap<String, u64>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT project,count(*) FROM tasks
             WHERE project IS NOT NULL AND status NOT IN ('done','dropped')
             GROUP BY project",
            )
            .context("prepare open project counts")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)?)))
            .context("query open project counts")?;
        rows.map(|row| {
            let (project, count) = row.context("read open project count")?;
            Ok((
                project,
                count
                    .try_into()
                    .context("project count must not be negative")?,
            ))
        })
        .collect()
    }

    pub fn closed_task_count(&self) -> Result<u64> {
        self.connection
            .query_row(
                "SELECT count(*) FROM tasks WHERE status IN ('done','dropped')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .context("count closed tasks")?
            .try_into()
            .context("closed task count must not be negative")
    }

    /// Newest updated first, newest task number first on ties. Canonical UTC
    /// timestamps compare chronologically once the trailing Z is removed.
    pub fn closed_tasks_before(
        &self,
        before: Option<(jiff::Timestamp, u32)>,
        limit: u16,
    ) -> Result<Vec<Task>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT document FROM tasks WHERE status IN ('done','dropped')
             AND (?1 IS NULL OR (rtrim(json_extract(document,'$.updated'),'Z'),CAST(substr(id,3) AS INTEGER)) < (?1,?2))
             ORDER BY rtrim(json_extract(document,'$.updated'),'Z') DESC,CAST(substr(id,3) AS INTEGER) DESC
             LIMIT ?3",
        ).context("prepare closed task page")?;
        let at = before.map(|(at, _)| at.to_string());
        let at = at.as_deref().map(|at| at.trim_end_matches('Z'));
        documents(&mut statement, params![at, before.map(|(_, id)| id), limit])
    }

    pub fn insert_run(&self, run: &Run) -> Result<()> {
        self.connection
            .execute(
                "INSERT INTO runs(task,id,role,live,document) VALUES(?1,?2,?3,?4,?5)",
                params![
                    run.task,
                    run.id,
                    run.role.as_str(),
                    run.ended.is_none(),
                    encode(run)?
                ],
            )
            .context("insert run")?;
        Ok(())
    }

    pub fn update_run(&self, run: &Run) -> Result<()> {
        let changed = self
            .connection
            .execute(
                "UPDATE runs SET role=?3,live=?4,document=?5 WHERE task=?1 AND id=?2",
                params![
                    run.task,
                    run.id,
                    run.role.as_str(),
                    run.ended.is_none(),
                    encode(run)?
                ],
            )
            .context("update run")?;
        ensure!(changed == 1, "run {}:{} does not exist", run.task, run.id);
        Ok(())
    }

    pub fn get_run(&self, task: &str, id: &str) -> Result<Option<Run>> {
        self.optional_document(
            "SELECT document FROM runs WHERE task=?1 AND id=?2",
            params![task, id],
        )
    }

    pub fn list_runs(&self, task: &str) -> Result<Vec<Run>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT document FROM runs WHERE task=?1 ORDER BY CAST(substr(id,2) AS INTEGER)",
            )
            .context("prepare run listing")?;
        documents(&mut statement, [task])
    }

    pub fn all_runs(&self) -> Result<Vec<Run>> {
        let mut statement = self
            .connection
            .prepare("SELECT document FROM runs ORDER BY task,CAST(substr(id,2) AS INTEGER)")
            .context("prepare all runs listing")?;
        documents(&mut statement, [])
    }

    pub fn latest_run(&self, task: &str, role: Role) -> Result<Option<Run>> {
        self.optional_document("SELECT document FROM runs WHERE task=?1 AND role=?2 ORDER BY CAST(substr(id,2) AS INTEGER) DESC LIMIT 1", params![task, role.as_str()])
    }

    pub fn live_runs(&self, role: Option<Role>) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare("SELECT document FROM runs WHERE live=1 AND (?1 IS NULL OR role=?1) ORDER BY task,CAST(substr(id,2) AS INTEGER)").context("prepare live run listing")?;
        documents(&mut statement, [role.map(Role::as_str)])
    }

    pub fn live_runs_for_task(&self, task: &str) -> Result<Vec<Run>> {
        let mut statement = self.connection.prepare("SELECT document FROM runs WHERE task=?1 AND live=1 ORDER BY CAST(substr(id,2) AS INTEGER)").context("prepare task live runs")?;
        documents(&mut statement, [task])
    }

    pub(crate) fn occupied_slots(&self, project: Option<&str>) -> Result<usize> {
        let count: i64 = self.connection.query_row("SELECT count(*) FROM tasks WHERE status IN ('running','blocked') AND (?1 IS NULL OR project=?1)", [project], |row| row.get(0)).context("count tasks holding a dispatch slot")?;
        count.try_into().context("slot count exceeds platform size")
    }

    /// Call inside the run-insertion transaction; IDs are never reused.
    pub fn next_run_id(&self, task: &str) -> Result<String> {
        let number: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE(MAX(CAST(substr(id,2) AS INTEGER)),0)+1 FROM runs WHERE task=?1",
                [task],
                |row| row.get(0),
            )
            .context("allocate next run id")?;
        Ok(format!("R{number}"))
    }

    pub fn insert_check(&self, check: &Check) -> Result<()> {
        self.connection
            .execute(
                "INSERT INTO checks(task,kind,key,document) VALUES(?1,?2,?3,?4)",
                params![check.task, check.kind.as_str(), check.key, encode(check)?],
            )
            .context("insert check")?;
        Ok(())
    }

    /// Insertion order, not agent-provided timestamps, decides the newest check.
    pub fn latest_check(&self, task: &str, kind: CheckKind, key: &str) -> Result<Option<Check>> {
        self.optional_document("SELECT document FROM checks WHERE task=?1 AND kind=?2 AND key=?3 ORDER BY seq DESC LIMIT 1", params![task, kind.as_str(), key])
    }

    pub fn has_gate_checks(&self, task: &str) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM checks WHERE task=?1 AND kind='gate')",
                [task],
                |row| row.get(0),
            )
            .context("check for recorded gate results")
    }

    pub fn list_checks(&self, task: &str) -> Result<Vec<Check>> {
        let mut statement = self
            .connection
            .prepare("SELECT document FROM checks WHERE task=?1 ORDER BY seq")
            .context("prepare check listing")?;
        documents(&mut statement, [task])
    }

    pub(crate) fn list_review_checks(&self, task: &str) -> Result<Vec<Check>> {
        let mut statement = self
            .connection
            .prepare("SELECT document FROM checks WHERE task=?1 AND kind='review' ORDER BY seq")
            .context("prepare review history listing")?;
        documents(&mut statement, [task])
    }

    /// Newest result for each gate commit or review round, most recent first.
    pub fn list_latest_checks(&self, task: &str) -> Result<Vec<Check>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT document FROM checks WHERE seq IN (
                SELECT max(seq) FROM checks WHERE task=?1 GROUP BY kind,key
             ) ORDER BY seq DESC",
            )
            .context("prepare newest check listing")?;
        documents(&mut statement, [task])
    }

    fn optional_document<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<T>> {
        let document: Option<String> = self
            .connection
            .query_row(sql, params, |row| row.get(0))
            .optional()
            .context("query stored document")?;
        document
            .map(|text| serde_json::from_str(&text).context("decode stored document"))
            .transpose()
    }
}

pub(crate) fn encode(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(value).context("encode stored document")
}

pub(crate) fn documents<T: DeserializeOwned>(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let rows = statement
        .query_map(params, |row| row.get::<_, String>(0))
        .context("query stored documents")?;
    rows.map(|row| {
        serde_json::from_str(&row.context("read stored document")?)
            .context("decode stored document")
    })
    .collect()
}

fn validate_task(task: &Task) -> Result<()> {
    ensure!(
        (task.status == Status::Blocked) == task.blocked.is_some(),
        "blocked metadata must be present exactly when task status is blocked"
    );
    Ok(())
}

const SCHEMA: &str = "
BEGIN IMMEDIATE;
CREATE TABLE tasks(id TEXT PRIMARY KEY, status TEXT NOT NULL, project TEXT, document TEXT NOT NULL);
CREATE INDEX tasks_status ON tasks(status);
CREATE INDEX tasks_project ON tasks(project);
CREATE TABLE runs(task TEXT NOT NULL REFERENCES tasks(id), id TEXT NOT NULL, role TEXT NOT NULL, live INTEGER NOT NULL, document TEXT NOT NULL, PRIMARY KEY(task,id));
CREATE INDEX runs_live_role ON runs(live,role);
CREATE INDEX runs_id ON runs(id);
CREATE TABLE checks(seq INTEGER PRIMARY KEY AUTOINCREMENT, task TEXT NOT NULL REFERENCES tasks(id), kind TEXT NOT NULL, key TEXT NOT NULL, document TEXT NOT NULL);
CREATE INDEX checks_lookup ON checks(task,kind,key,seq);
CREATE TABLE artifacts(task TEXT NOT NULL REFERENCES tasks(id), ordinal INTEGER NOT NULL, sha256 TEXT NOT NULL, document TEXT NOT NULL, PRIMARY KEY(task,ordinal));
CREATE INDEX artifacts_sha256 ON artifacts(sha256);
CREATE TABLE events(seq INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, task TEXT REFERENCES tasks(id), run TEXT, kind TEXT NOT NULL, actor TEXT NOT NULL, detail TEXT NOT NULL);
CREATE INDEX events_task ON events(task,seq);
CREATE INDEX events_kind ON events(kind,seq);
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta(key,value) VALUES('next_task','1');
PRAGMA user_version=1;
COMMIT;
";

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
