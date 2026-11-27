use std::{path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use corral_types::board_plan::BoardPlanReceipt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
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
        let version = check_schema_version(&connection)?;
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
            .execute_batch(crate::repair_queue::SCHEMA)
            .context("initialize pending repair queue")?;
        connection
            .execute_batch(crate::usage::SCHEMA)
            .context("initialize usage table")?;
        connection
            .execute_batch("CREATE INDEX IF NOT EXISTS events_task_kind ON events(task,kind,seq)")
            .context("index task events by kind")?;
        // Git History finds the events naming a commit without scanning the log.
        connection
            .execute_batch(crate::git_history::INDEX)
            .context("index events by commit")?;
        connection
            .execute_batch(crate::gate_attempts::SCHEMA)
            .context("initialize gate attempt table")?;
        connection
            .execute_batch(crate::takeover::identity::SCHEMA)
            .context("initialize run checkout identity table")?;
        let store = Self { connection };
        crate::artifacts::prepare_catalog(&store)?;
        Ok(store)
    }

    /// Open an existing database without running schema setup or allowing writes.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("open read-only store {}", path.display()))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .context("configure SQLite busy timeout")?;
        check_schema_version(&connection)?;
        let store = Self { connection };
        crate::artifacts::prepare_read_only_catalog(&store)?;
        Ok(store)
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

    /// The receipt table is additive and created by the first successful filing.
    pub(crate) fn has_board_plan_receipts(&self) -> Result<bool> {
        self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name='board_plan_receipts')",
            [],
            |row| row.get(0),
        ).context("check for board plan receipt table")
    }

    pub(crate) fn board_plan_receipt(&self, id: &str) -> Result<Option<(String, String)>> {
        if !self.has_board_plan_receipts()? {
            return Ok(None);
        }
        self.connection
            .query_row(
                "SELECT request_hash, receipt FROM board_plan_receipts WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("look up board plan receipt")
    }

    /// The caller holds the same creating transaction as the task and event writes.
    pub(crate) fn record_board_plan_receipt(
        &self,
        id: &str,
        hash: &str,
        receipt: &BoardPlanReceipt,
    ) -> Result<()> {
        self.connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS board_plan_receipts (
                    id TEXT PRIMARY KEY, request_hash TEXT NOT NULL, receipt TEXT NOT NULL
                )",
            )
            .context("initialize board plan receipt table")?;
        self.connection
            .execute(
                "INSERT INTO board_plan_receipts(id,request_hash,receipt) VALUES(?1,?2,?3)",
                params![id, hash, encode(receipt)?],
            )
            .context("record board plan receipt")?;
        Ok(())
    }

    pub(crate) fn task_dependencies(&self, id: &str) -> Result<Option<Vec<String>>> {
        self.optional_document(
            "SELECT json_extract(document,'$.depends_on') FROM tasks WHERE id=?1",
            [id],
        )
    }

    /// Read the ID sequence without consuming an ID.
    pub fn next_task_number(&self) -> Result<u64> {
        self.connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key='next_task'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .context("read next task number")?
            .try_into()
            .context("next task number must not be negative")
    }

    /// Reserve the exported ID range without rewinding a more advanced sequence.
    pub fn reserve_task_ids_through(&self, last: u32) -> Result<()> {
        self.connection
            .execute(
                "UPDATE meta SET value=?1 WHERE key='next_task' AND CAST(value AS INTEGER) < ?1",
                [i64::from(last) + 1],
            )
            .context("reserve exported task IDs")?;
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

    /// Read existing tasks for a set of IDs without one query per task.
    pub fn tasks_by_ids(&self, ids: &[&str]) -> Result<Vec<Task>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT document FROM tasks WHERE id IN (SELECT value FROM json_each(?1))",
            )
            .context("prepare tasks by IDs")?;
        documents(&mut statement, [encode(&ids)?])
    }

    /// Read the named tasks and their ancestors in one batch. Limit the walk to eight parent
    /// hops so even cyclic data stays bounded.
    pub fn tasks_with_ancestors(&self, ids: &[&str]) -> Result<Vec<Task>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare_cached(
                "WITH RECURSIVE ancestors(id, depth) AS (
                     SELECT value, 0 FROM json_each(?1)
                     UNION
                     SELECT json_extract(tasks.document,'$.parent'), ancestors.depth + 1
                     FROM ancestors JOIN tasks ON tasks.id=ancestors.id
                     WHERE ancestors.depth < 8
                 )
                 SELECT document FROM tasks WHERE id IN (SELECT id FROM ancestors)",
            )
            .context("prepare tasks with ancestors")?;
        documents(&mut statement, [encode(&ids)?])
    }

    /// Find direct children of all named parents in a single scan of the task documents.
    pub fn children_of(&self, parents: &[&str]) -> Result<Vec<Task>> {
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT document FROM tasks
                 WHERE json_extract(document,'$.parent') IN (SELECT value FROM json_each(?1))",
            )
            .context("prepare task children")?;
        documents(&mut statement, [encode(&parents)?])
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

    /// Find draft aliases without treating an already-retargeted repair as a draft.
    pub(crate) fn open_draft_lens_task(
        &self,
        project: &str,
        keys: &[String],
    ) -> Result<Option<Task>> {
        self.optional_document(
            "SELECT t.document FROM tasks AS t
             WHERE t.project=?1 AND t.status NOT IN ('done','dropped') AND EXISTS (
                 SELECT 1 FROM events AS e, json_each(e.detail, '$.lens_task_keys') AS key
                 WHERE e.task=t.id AND e.kind='created'
                   AND key.value IN (SELECT value FROM json_each(?2))
             ) AND NOT EXISTS (
                 SELECT 1 FROM events AS e, json_each(e.detail, '$.lens_task_keys') AS key
                 WHERE e.task=t.id AND e.kind IN ('created','edited')
                   AND key.value LIKE 'repair-file:%'
             ) ORDER BY t.rowid LIMIT 1",
            params![project, encode(&keys)?],
        )
    }

    /// Lens keys are recorded on creation or explicit draft-to-repair retargeting,
    /// independent of later changes to titles or body text.
    /// Call under the same writer transaction as insertion.
    pub(crate) fn open_lens_task(&self, project: &str, keys: &[String]) -> Result<Option<Task>> {
        self.optional_document(
            "SELECT t.document FROM tasks AS t
             WHERE t.project=?1 AND t.status NOT IN ('done','dropped') AND EXISTS (
                 SELECT 1 FROM events AS e, json_each(e.detail, '$.lens_task_keys') AS key
                 WHERE e.task=t.id AND e.kind IN ('created','edited')
                   AND key.value IN (SELECT value FROM json_each(?2))
             ) ORDER BY t.rowid LIMIT 1",
            params![project, encode(&keys)?],
        )
    }

    /// Fetch status alone for a set of task IDs, including finished tasks.
    pub fn task_statuses(
        &self,
        ids: &std::collections::BTreeSet<&str>,
    ) -> Result<std::collections::BTreeMap<String, Status>> {
        if ids.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }
        let mut statement = self.connection.prepare_cached(
            "SELECT tasks.id,tasks.status FROM json_each(?1) JOIN tasks ON tasks.id=json_each.value",
        ).context("prepare task statuses")?;
        let rows = statement
            .query_map([encode(ids)?], |row| {
                Ok((row.get(0)?, row.get::<_, String>(1)?))
            })
            .context("query task statuses")?;
        rows.map(|row| {
            let (id, status) = row.context("read task status row")?;
            Ok((
                id,
                Status::parse(&status).context("parse stored task status")?,
            ))
        })
        .collect()
    }

    /// Per-project open counts grouped by status. Closed tasks and filed plan parents, which
    /// summarize their children rather than being work, are not counted.
    pub fn open_project_status_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<String, Vec<(Status, u64)>>> {
        let summaries = self.not_filed_parent()?;
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT project,status,count(*) FROM tasks
             WHERE project IS NOT NULL AND status NOT IN ('done','dropped') {summaries}
             GROUP BY project,status",
            ))
            .context("prepare project counts by status")?;
        let mut rows = statement
            .query([])
            .context("query project counts by status")?;
        let mut counts = std::collections::BTreeMap::<_, Vec<_>>::new();
        while let Some(row) = rows.next().context("read project status count row")? {
            let project = row.get(0).context("read project key")?;
            let status: String = row.get(1).context("read project task status")?;
            let count: i64 = row.get(2).context("read project task count")?;
            counts.entry(project).or_default().push((
                Status::parse(&status).context("parse stored task status")?,
                count
                    .try_into()
                    .context("project task count must not be negative")?,
            ));
        }
        Ok(counts)
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

    /// Filed plan parents are left out: they list as plan summaries, whatever their status.
    fn not_filed_parent(&self) -> Result<String> {
        Ok(if self.has_board_plan_receipts()? {
            format!("AND id NOT IN ({})", crate::board_plan::FILED_PARENTS)
        } else {
            String::new()
        })
    }

    /// Closed tasks, without filed plan parents.
    pub fn closed_task_count(&self) -> Result<u64> {
        self.connection
            .query_row(
                &format!(
                    "SELECT count(*) FROM tasks WHERE status IN ('done','dropped') {}",
                    self.not_filed_parent()?
                ),
                [],
                |row| row.get::<_, i64>(0),
            )
            .context("count closed tasks")?
            .try_into()
            .context("closed task count must not be negative")
    }

    /// Newest updated first, newest task number first on ties. Canonical UTC
    /// timestamps compare chronologically once the trailing Z is removed. Filed plan
    /// parents are left out, as in [`Self::closed_task_count`].
    pub fn closed_tasks_before(
        &self,
        before: Option<(jiff::Timestamp, u32)>,
        limit: u16,
    ) -> Result<Vec<Task>> {
        let mut statement = self.connection.prepare_cached(&format!(
            "SELECT document FROM tasks WHERE status IN ('done','dropped') {}
             AND (?1 IS NULL OR (rtrim(json_extract(document,'$.updated'),'Z'),CAST(substr(id,3) AS INTEGER)) < (?1,?2))
             ORDER BY rtrim(json_extract(document,'$.updated'),'Z') DESC,CAST(substr(id,3) AS INTEGER) DESC
             LIMIT ?3",
            self.not_filed_parent()?
        )).context("prepare closed task page")?;
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

    /// Count task slots according to the same rule used by dispatch.
    pub fn occupied_slots(&self, project: Option<&str>) -> Result<usize> {
        let [running, blocked, review] = Status::SLOT_HOLDERS.map(Status::as_str);
        let count: i64 = self.connection.query_row("SELECT count(*) FROM tasks WHERE status IN (?2,?3,?4) AND (?1 IS NULL OR project=?1)", params![project, running, blocked, review], |row| row.get(0)).context("count tasks holding a dispatch slot")?;
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

    pub(crate) fn optional_document<T: DeserializeOwned>(
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

fn check_schema_version(connection: &Connection) -> Result<u32> {
    let version = connection
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
    Ok(version)
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
