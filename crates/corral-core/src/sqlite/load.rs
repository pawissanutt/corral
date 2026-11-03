//! Row decoders shared by the write path and the read view.

use anyhow::{Context, Result};
use rusqlite::{Connection, Params, Row, params};

use super::codec::*;
use crate::backend::*;
use crate::blob_ref::BlobHash;

#[cfg(test)]
thread_local! {
    pub(super) static VM_STEPS: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

fn measured(stmt: &rusqlite::Statement<'_>) {
    #[cfg(test)]
    VM_STEPS.with(|steps| {
        steps.set(steps.get() + stmt.reset_status(rusqlite::StatementStatus::VmStep));
    });
    #[cfg(not(test))]
    let _ = stmt;
}

pub(super) fn first<T>(
    db: &Connection,
    sql: &str,
    params: impl Params,
    map: impl FnOnce(&Row<'_>) -> Result<T>,
) -> Result<Option<T>> {
    let mut stmt = db
        .prepare_cached(sql)
        .with_context(|| format!("prepare {sql}"))?;
    let mut rows = stmt.query(params)?;
    let out = match rows.next()? {
        Some(row) => Some(map(row)?),
        None => None,
    };
    drop(rows);
    measured(&stmt);
    Ok(out)
}

pub(super) fn all<T>(
    db: &Connection,
    sql: &str,
    params: impl Params,
    mut map: impl FnMut(&Row<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = db
        .prepare_cached(sql)
        .with_context(|| format!("prepare {sql}"))?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map(row)?);
    }
    drop(rows);
    measured(&stmt);
    Ok(out)
}

/// At most `limit` rows of `sql`, decoded, and whether another row follows; the following row
/// is stepped to but never decoded.
pub(super) fn up_to<T>(
    db: &Connection,
    sql: &str,
    params: impl Params,
    limit: usize,
    mut map: impl FnMut(&Row<'_>) -> Result<T>,
) -> Result<(Vec<T>, bool)> {
    let mut stmt = db
        .prepare_cached(sql)
        .with_context(|| format!("prepare {sql}"))?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    let mut more = false;
    while let Some(row) = rows.next()? {
        if out.len() == limit {
            more = true;
            break;
        }
        out.push(map(row)?);
    }
    drop(rows);
    measured(&stmt);
    Ok((out, more))
}

pub(super) fn text(row: &Row<'_>, i: usize) -> Result<String> {
    Ok(row.get(i)?)
}

pub(super) fn opt_text(row: &Row<'_>, i: usize) -> Result<Option<String>> {
    Ok(row.get(i)?)
}

pub(super) fn num(row: &Row<'_>, i: usize) -> Result<u64> {
    uint(row.get(i)?)
}

pub(super) fn opt_num(row: &Row<'_>, i: usize) -> Result<Option<u64>> {
    row.get::<_, Option<i64>>(i)?.map(uint).transpose()
}

pub(super) fn flag(row: &Row<'_>, i: usize) -> Result<bool> {
    Ok(row.get::<_, i64>(i)? != 0)
}

pub(super) fn time(row: &Row<'_>, i: usize) -> Result<jiff::Timestamp> {
    parse_ts(&text(row, i)?)
}

pub(super) fn opt_time(row: &Row<'_>, i: usize) -> Result<Option<jiff::Timestamp>> {
    parse_opt_ts(opt_text(row, i)?)
}

pub(super) fn doc(row: &Row<'_>, i: usize) -> Result<serde_json::Value> {
    parse_json(&text(row, i)?)
}

pub(super) const HEAD_COLUMNS: &str = "num, id, rev, created, title, status, project, priority, \
     parent, updated, state, history, kind, size, tracker, design, queued, spec_rev, dashboard_due, \
     spec, body";

/// The columns of a task header: no stored document among them.
pub(super) const HEADER_COLUMNS: &str = "num, id, rev, created, title, status, project, \
     priority, parent, updated, kind, size, tracker, design, queued, spec_rev, dashboard_due";

/// A task header from [`HEADER_COLUMNS`], without decoding the head document or summary.
pub(super) fn header(db: &Connection, row: &Row<'_>) -> Result<TaskHeader> {
    let num = TaskNum(num(row, 0)?);
    Ok(TaskHeader {
        num,
        id: text(row, 1)?,
        rev: self::num(row, 2)?,
        created: time(row, 3)?,
        fields: TaskFields {
            title: text(row, 4)?,
            status: text(row, 5)?,
            project: opt_text(row, 6)?,
            priority: text(row, 7)?,
            parent: opt_text(row, 8)?,
            updated: time(row, 9)?,
            edges: edges(db, num)?,
            kind: opt_text(row, 10)?,
            size: opt_text(row, 11)?,
            tracker: flag(row, 12)?,
            design: flag(row, 13)?,
            queued: flag(row, 14)?,
            spec_rev: text(row, 15)?,
            dashboard_due: opt_time(row, 16)?,
        },
    })
}

pub(super) fn head(db: &Connection, row: &Row<'_>) -> Result<TaskHeadDoc> {
    let num = TaskNum(num(row, 0)?);
    Ok(TaskHeadDoc {
        header: TaskHeader {
            num,
            id: text(row, 1)?,
            rev: self::num(row, 2)?,
            created: time(row, 3)?,
            fields: TaskFields {
                title: text(row, 4)?,
                status: text(row, 5)?,
                project: opt_text(row, 6)?,
                priority: text(row, 7)?,
                parent: opt_text(row, 8)?,
                updated: time(row, 9)?,
                edges: edges(db, num)?,
                kind: opt_text(row, 12)?,
                size: opt_text(row, 13)?,
                tracker: flag(row, 14)?,
                design: flag(row, 15)?,
                queued: flag(row, 16)?,
                spec_rev: text(row, 17)?,
                dashboard_due: opt_time(row, 18)?,
            },
        },
        doc: join_head(&text(row, 19)?, &text(row, 10)?, &text(row, 20)?)?,
        summary: doc(row, 11)?,
    })
}

/// The `spec`, `state` and `body` columns of a head document: its `spec` and `body` members
/// (empty text when absent) and everything else.
pub(super) struct HeadColumns {
    pub(super) spec: String,
    pub(super) state: String,
    pub(super) body: String,
}

pub(super) fn split_head(doc: &serde_json::Value) -> Result<HeadColumns> {
    let Some(object) = doc.as_object() else {
        return Ok(HeadColumns {
            spec: String::new(),
            state: json(doc)?,
            body: String::new(),
        });
    };
    let member = |name: &str| object.get(name).map(json).transpose();
    let mut state = object.clone();
    state.remove("spec");
    state.remove("body");
    Ok(HeadColumns {
        spec: member("spec")?.unwrap_or_default(),
        state: json(&serde_json::Value::Object(state))?,
        body: member("body")?.unwrap_or_default(),
    })
}

fn join_head(spec: &str, state: &str, body: &str) -> Result<serde_json::Value> {
    let mut doc = parse_json::<serde_json::Value>(state)?;
    if let Some(object) = doc.as_object_mut() {
        if !spec.is_empty() {
            object.insert("spec".to_owned(), parse_json(spec)?);
        }
        if !body.is_empty() {
            object.insert("body".to_owned(), parse_json(body)?);
        }
    }
    Ok(doc)
}

pub(super) fn head_by_num(db: &Connection, task: TaskNum) -> Result<Option<TaskHeadDoc>> {
    let sql = format!("SELECT {HEAD_COLUMNS} FROM tasks WHERE num = ?1");
    first(db, &sql, params![int(task.0)?], |row| head(db, row))
}

/// Edges in the order the last write gave them.
pub(super) fn edges(db: &Connection, task: TaskNum) -> Result<Vec<TaskEdge>> {
    all(
        db,
        "SELECT kind, other FROM task_edges WHERE task = ?1 ORDER BY rowid",
        params![int(task.0)?],
        |row| {
            Ok(TaskEdge {
                kind: text(row, 0)?,
                other: text(row, 1)?,
            })
        },
    )
}

pub(super) fn history_row(row: &Row<'_>) -> Result<HistoryRow> {
    Ok(HistoryRow {
        part: from_name(&text(row, 0)?)?,
        key: text(row, 1)?,
        current: flag(row, 2)?,
        open: flag(row, 3)?,
        doc: doc(row, 4)?,
        index: parse_json(&text(row, 5)?)?,
    })
}

/// The selected rows of one task in `(part, key)` order.
pub(super) fn rows(db: &Connection, task: TaskNum, parts: &TaskParts) -> Result<Vec<HistoryRow>> {
    let mut rows = all(
        db,
        "SELECT part, key, current, open, doc, idx FROM task_rows WHERE task = ?1",
        params![int(task.0)?],
        history_row,
    )?;
    rows.retain(|row| parts.includes(row.part));
    rows.sort_by(|a, b| (a.part, &a.key).cmp(&(b.part, &b.key)));
    Ok(rows)
}

pub(super) const BLOB_COLUMNS: &str =
    "hash, raw_bytes, stored_bytes, codec, state, refs, unreferenced_since, created";

pub(super) fn blob(db: &Connection, row: &Row<'_>) -> Result<BlobRow> {
    let hash = BlobHash::parse(&text(row, 0)?)?;
    let reservations = all(
        db,
        "SELECT token, owner, expires FROM reservations WHERE hash = ?1 ORDER BY rowid",
        params![hash.as_str()],
        |row| {
            Ok(Reservation {
                token: text(row, 0)?,
                owner: text(row, 1)?,
                expires: time(row, 2)?,
            })
        },
    )?;
    Ok(BlobRow {
        hash,
        raw_bytes: num(row, 1)?,
        stored_bytes: num(row, 2)?,
        codec: text(row, 3)?,
        state: from_name(&text(row, 4)?)?,
        refs: num(row, 5)?,
        unreferenced_since: opt_time(row, 6)?,
        created: time(row, 7)?,
        reservations,
    })
}

pub(super) fn blob_by_hash(db: &Connection, hash: &BlobHash) -> Result<Option<BlobRow>> {
    let sql = format!("SELECT {BLOB_COLUMNS} FROM blobs WHERE hash = ?1");
    first(db, &sql, params![hash.as_str()], |row| blob(db, row))
}

pub(super) const SCRATCH_COLUMNS: &str = "id, kind, task, subject, attempt, path, state, reason, \
     next_check_at, owner, warned, evidence_bytes, created, closed, reclaimed";

pub(super) fn scratch(row: &Row<'_>) -> Result<ScratchRow> {
    Ok(ScratchRow {
        id: text(row, 0)?,
        kind: from_name(&text(row, 1)?)?,
        task: opt_text(row, 2)?,
        subject: opt_text(row, 3)?,
        attempt: opt_text(row, 4)?,
        path: text(row, 5)?,
        state: from_name(&text(row, 6)?)?,
        reason: opt_text(row, 7)?,
        next_check_at: opt_time(row, 8)?,
        owner: opt_text(row, 9)?,
        warned: opt_time(row, 10)?,
        evidence_bytes: num(row, 11)?,
        created: time(row, 12)?,
        closed: opt_time(row, 13)?,
        reclaimed: opt_time(row, 14)?,
    })
}

pub(super) fn scratch_by_id(db: &Connection, id: &str) -> Result<Option<ScratchRow>> {
    let sql = format!("SELECT {SCRATCH_COLUMNS} FROM scratch_dirs WHERE id = ?1");
    first(db, &sql, params![id], scratch)
}

pub(super) const LIFECYCLE_COLUMNS: &str =
    "task, generation, target, policy_digest, outcome, reason, bytes, next_check_at, at";

pub(super) fn lifecycle(row: &Row<'_>) -> Result<LifecycleRow> {
    Ok(LifecycleRow {
        task: text(row, 0)?,
        generation: text(row, 1)?,
        target: text(row, 2)?,
        policy_digest: text(row, 3)?,
        outcome: from_name(&text(row, 4)?)?,
        reason: opt_text(row, 5)?,
        bytes: opt_num(row, 6)?,
        next_check_at: opt_time(row, 7)?,
        at: time(row, 8)?,
    })
}

pub(super) fn lease(row: &Row<'_>) -> Result<LeaseRow> {
    Ok(LeaseRow {
        id: text(row, 0)?,
        holder: text(row, 1)?,
        started: time(row, 2)?,
        expires: time(row, 3)?,
    })
}

pub(super) fn claim(row: &Row<'_>) -> Result<ClaimRow> {
    Ok(ClaimRow {
        name: text(row, 0)?,
        owner: text(row, 1)?,
        acquired: time(row, 2)?,
        doc: doc(row, 3)?,
    })
}

pub(super) fn authorization(row: &Row<'_>) -> Result<AuthorizationRow> {
    Ok(AuthorizationRow {
        id: text(row, 0)?,
        task: text(row, 1)?,
        disposition: from_name(&text(row, 2)?)?,
        doc: doc(row, 3)?,
    })
}

pub(super) const CONDITION_COLUMNS: &str = "opened_seq, task, kind, detail_digest, count, \
     first_at, last_at, first_actor, first_via, last_actor, last_via, closed_seq, close_reason";

pub(super) fn condition_run(row: &Row<'_>) -> Result<ConditionRun> {
    Ok(ConditionRun {
        opened_seq: EventSeq(num(row, 0)?),
        task: text(row, 1)?,
        kind: text(row, 2)?,
        detail_digest: text(row, 3)?,
        count: num(row, 4)?,
        first_at: time(row, 5)?,
        last_at: time(row, 6)?,
        first_actor: opt_text(row, 7)?,
        first_via: opt_text(row, 8)?,
        last_actor: opt_text(row, 9)?,
        last_via: opt_text(row, 10)?,
        closed_seq: opt_num(row, 11)?.map(EventSeq),
        close_reason: opt_text(row, 12)?.as_deref().map(from_name).transpose()?,
    })
}

pub(super) fn record(row: &Row<'_>) -> Result<StoredRecord> {
    Ok(StoredRecord {
        area: text(row, 0)?,
        key: text(row, 1)?,
        version: num(row, 2)?,
        doc: doc(row, 3)?,
        updated: time(row, 4)?,
    })
}

pub(super) const EVENT_COLUMNS: &str = "seq, ts, task, kind, actor, via, run, detail, malformed";

pub(super) fn event(row: &Row<'_>) -> Result<StoredEvent> {
    let detail = text(row, 7)?;
    crate::work::decoded(
        crate::work::Counter::EventDecodes,
        u64::try_from(detail.len())?,
        crate::work::Subject::Plain,
    );
    Ok(StoredEvent {
        seq: EventSeq(num(row, 0)?),
        event: NewEvent {
            ts: time(row, 1)?,
            task: opt_text(row, 2)?,
            kind: text(row, 3)?,
            actor: opt_text(row, 4)?,
            via: opt_text(row, 5)?,
            run: opt_text(row, 6)?,
            detail: parse_json(&detail)?,
        },
        malformed: flag(row, 8)?,
    })
}

pub(super) fn meta_u64(db: &Connection, key: &str) -> Result<u64> {
    let value = first(
        db,
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |row| text(row, 0),
    )?
    .with_context(|| format!("meta {key} is missing"))?;
    value
        .parse()
        .with_context(|| format!("meta {key} = {value:?}"))
}

pub(super) fn status_counts(db: &Connection) -> Result<Vec<StatusCount>> {
    all(
        db,
        "SELECT status, project, n FROM status_counts ORDER BY status, project",
        [],
        |row| {
            Ok(StatusCount {
                status: text(row, 0)?,
                project: text(row, 1)?,
                n: num(row, 2)?,
            })
        },
    )
}
