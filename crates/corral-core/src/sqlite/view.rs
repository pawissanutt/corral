//! The read view: one deferred read transaction on a pooled connection, pinned when opened by
//! reading `meta.change_seq`, with an age check before every read. Dropping the view ends the
//! transaction and returns the connection, which releases its WAL snapshot.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::anyhow;
use jiff::Timestamp;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, Row, params, params_from_iter};

use super::codec::*;
use super::load;
use crate::backend::*;

pub(super) struct SqliteView {
    conn: Option<Connection>,
    pool: Arc<Mutex<Vec<Connection>>>,
    seq: ChangeSeq,
    floor: ChangeSeq,
    opened: Timestamp,
    clock: Arc<dyn Clock>,
    max_age: Duration,
    _retain: Option<super::Keepalive>,
}

impl SqliteView {
    pub(super) fn open(
        conn: Connection,
        pool: Arc<Mutex<Vec<Connection>>>,
        clock: Arc<dyn Clock>,
        max_age: Duration,
        retain: Option<super::Keepalive>,
    ) -> Result<Self, ViewError> {
        conn.execute_batch("BEGIN DEFERRED")?;
        let pinned = load::meta_u64(&conn, "change_seq")
            .and_then(|seq| Ok((seq, load::meta_u64(&conn, "feed_floor")?)));
        let (seq, floor) = match pinned {
            Ok(pinned) => pinned,
            Err(error) => {
                drop(conn);
                return Err(ViewError::Io(error));
            }
        };
        Ok(Self {
            conn: Some(conn),
            pool,
            seq: ChangeSeq(seq),
            floor: ChangeSeq(floor),
            opened: clock.now(),
            clock,
            max_age,
            _retain: retain,
        })
    }

    fn db(&self) -> Result<&Connection, ViewError> {
        self.conn
            .as_ref()
            .ok_or_else(|| ViewError::Io(anyhow!("read view connection is closed")))
    }

    fn fresh(&self) -> Result<&Connection, ViewError> {
        let age = elapsed(self.opened, self.clock.now());
        if age > self.max_age {
            return Err(ViewError::Expired { age });
        }
        self.db()
    }

    fn page<T>(
        &self,
        req: &PageReq,
        sql: &str,
        mut args: Vec<Sql>,
        mut map: impl FnMut(&Connection, &Row<'_>) -> anyhow::Result<T>,
    ) -> Result<Page<T>, ViewError> {
        let db = self.fresh()?;
        let (offset, limit) = bounds(req)?;
        args.push(Sql::Integer(int(limit as u64 + 1).or_view()?));
        args.push(Sql::Integer(int(offset as u64).or_view()?));
        let sql = format!("{sql} LIMIT ? OFFSET ?");
        let (items, more) =
            load::up_to(db, &sql, params_from_iter(args), limit, |row| map(db, row)).or_view()?;
        let next = more.then(|| (offset + limit).to_string());
        Ok(Page { items, next })
    }

    fn check_feed(&self, after: ChangeSeq) -> Result<&Connection, FeedError> {
        let db = self.fresh()?;
        if after < self.floor {
            return Err(FeedError::Gap {
                oldest: ChangeSeq(self.floor.0 + 1),
            });
        }
        Ok(db)
    }

    fn open_head(&self, db: &Connection, num: TaskNum) -> anyhow::Result<Option<TaskHeadDoc>> {
        Ok(load::head_by_num(db, num)?.filter(|head| head.header.is_open()))
    }
}

impl Drop for SqliteView {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take()
            && conn.execute_batch("ROLLBACK").is_ok()
        {
            self.pool
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(conn);
        }
    }
}

fn bounds(req: &PageReq) -> Result<(usize, usize), ViewError> {
    if req.limit == 0 {
        return Err(ViewError::Io(anyhow!("page limit must be positive")));
    }
    let offset = match &req.after {
        None => 0,
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| ViewError::Io(anyhow!("invalid page cursor {cursor:?}")))?,
    };
    Ok((offset, req.limit as usize))
}

fn page_vec<T>(items: Vec<T>, req: &PageReq) -> Result<Page<T>, ViewError> {
    let (offset, limit) = bounds(req)?;
    let mut items: Vec<T> = items.into_iter().skip(offset).take(limit + 1).collect();
    let next = (items.len() > limit).then(|| {
        items.truncate(limit);
        (offset + limit).to_string()
    });
    Ok(Page { items, next })
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

fn text_args(values: &[String]) -> impl Iterator<Item = Sql> + '_ {
    values.iter().map(|value| Sql::Text(value.clone()))
}

fn changed_tasks(db: &Connection, since: ChangeSeq) -> anyhow::Result<BTreeSet<TaskNum>> {
    let scopes = load::all(
        db,
        "SELECT key FROM change_scopes WHERE kind = 'task' AND seq > ?1",
        params![int(since.0)?],
        |row| parse_json::<ChangeScope>(&load::text(row, 0)?),
    )?;
    Ok(scopes
        .into_iter()
        .filter_map(|scope| match scope {
            ChangeScope::Task { num, .. } => Some(num),
            _ => None,
        })
        .collect())
}

fn head_query(db: &Connection, row: &Row<'_>) -> anyhow::Result<TaskHeadDoc> {
    load::head(db, row)
}

impl ReadView for SqliteView {
    fn seq(&self) -> ChangeSeq {
        self.seq
    }

    fn ensure_fresh(&self) -> Result<(), ViewError> {
        self.fresh().map(drop)
    }

    fn board(&self, since: Option<ChangeSeq>) -> Result<BoardDelta, FeedError> {
        let db = self.fresh()?;
        let (heads, left) = match since {
            None => {
                let sql = format!(
                    "SELECT {} FROM tasks WHERE status NOT IN ('done','dropped') ORDER BY num",
                    load::HEAD_COLUMNS
                );
                let heads = load::all(db, &sql, [], |row| load::head(db, row)).or_view()?;
                (heads, Vec::new())
            }
            Some(since) => {
                self.check_feed(since)?;
                let mut heads = Vec::new();
                let mut left = Vec::new();
                for num in changed_tasks(db, since).or_view()? {
                    match self.open_head(db, num).or_view()? {
                        Some(head) => heads.push(head),
                        None => left.push(num),
                    }
                }
                (heads, left)
            }
        };
        Ok(BoardDelta {
            seq: self.seq,
            since,
            heads,
            left,
            status_counts: load::status_counts(db).or_view()?,
        })
    }

    fn changes(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage, FeedError> {
        let db = self.check_feed(after)?;
        if limit == 0 {
            return Err(FeedError::View(ViewError::Io(anyhow!(
                "page limit must be positive"
            ))));
        }
        let mut entries = load::all(
            db,
            "SELECT seq, at, op FROM changes WHERE seq > ?1 ORDER BY seq LIMIT ?2",
            params![
                int(after.0).or_view()?,
                int(u64::from(limit) + 1).or_view()?
            ],
            |row| {
                let seq = ChangeSeq(load::num(row, 0)?);
                let mut scopes = load::all(
                    db,
                    "SELECT key FROM change_scopes WHERE seq = ?1",
                    params![int(seq.0)?],
                    |row| parse_json::<ChangeScope>(&load::text(row, 0)?),
                )?;
                scopes.sort();
                Ok(ChangeEntry {
                    seq,
                    at: load::time(row, 1)?,
                    op: from_name(&load::text(row, 2)?)?,
                    scopes,
                })
            },
        )
        .or_view()?;
        let more = entries.len() > limit as usize;
        entries.truncate(limit as usize);
        Ok(ChangePage { entries, more })
    }

    fn head(&self, num: TaskNum) -> Result<Option<TaskHeadDoc>, ViewError> {
        let db = self.fresh()?;
        load::head_by_num(db, num).or_view()
    }

    fn heads(&self, q: &HeadQuery, page: PageReq) -> Result<Page<TaskHeadDoc>, ViewError> {
        let cols = load::HEAD_COLUMNS;
        match q {
            HeadQuery::Open { project: None } => self.page(
                &page,
                &format!(
                    "SELECT {cols} FROM tasks WHERE status NOT IN ('done','dropped') ORDER BY num"
                ),
                Vec::new(),
                head_query,
            ),
            HeadQuery::Open {
                project: Some(project),
            } => self.page(
                &page,
                &format!(
                    "SELECT {cols} FROM tasks WHERE status NOT IN ('done','dropped') \
                     AND project = ? ORDER BY num"
                ),
                vec![Sql::Text(project.clone())],
                head_query,
            ),
            HeadQuery::Ids(ids) => {
                let db = self.fresh()?;
                let sql = format!("SELECT {cols} FROM tasks WHERE id = ?1");
                let mut found = BTreeMap::new();
                for id in ids.iter().collect::<BTreeSet<_>>() {
                    if let Some(head) =
                        load::first(db, &sql, params![id], |row| load::head(db, row)).or_view()?
                    {
                        found.insert(head.header.num, head);
                    }
                }
                page_vec(found.into_values().collect(), &page)
            }
            HeadQuery::Children { parent } => self.page(
                &page,
                &format!("SELECT {cols} FROM tasks WHERE parent = ? ORDER BY num"),
                vec![Sql::Text(parent.clone())],
                head_query,
            ),
            HeadQuery::ProjectCreatedAfter { project, after } => self.page(
                &page,
                &format!(
                    "SELECT {cols} FROM tasks WHERE project = ? AND created > ? \
                     ORDER BY created, id"
                ),
                vec![Sql::Text(project.clone()), Sql::Text(ts(*after))],
                head_query,
            ),
            HeadQuery::EdgeTo { kind, other } => self.page(
                &page,
                &format!(
                    "SELECT {cols} FROM tasks WHERE num IN \
                     (SELECT task FROM task_edges WHERE other = ? AND kind = ?) ORDER BY num"
                ),
                vec![Sql::Text(other.clone()), Sql::Text(kind.clone())],
                head_query,
            ),
            HeadQuery::RecentByStatus { status } => self.page(
                &page,
                &format!(
                    "SELECT {cols} FROM tasks WHERE status = ? ORDER BY updated DESC, num DESC"
                ),
                vec![Sql::Text(status.clone())],
                head_query,
            ),
        }
    }

    fn task(&self, num: TaskNum, parts: &TaskParts) -> Result<Option<TaskDoc>, ViewError> {
        let db = self.fresh()?;
        let Some(head) = load::head_by_num(db, num).or_view()? else {
            return Ok(None);
        };
        let rows = load::rows(db, num, parts).or_view()?;
        Ok(Some(TaskDoc { head, rows }))
    }

    fn rows(
        &self,
        num: TaskNum,
        q: &RowQuery,
        page: PageReq,
    ) -> Result<Page<HistoryRow>, ViewError> {
        let mut filters = vec!["r.task = ?".to_owned(), "r.part = ?".to_owned()];
        let mut args: Vec<Sql> = vec![
            Sql::Integer(int(num.0).or_view()?),
            Sql::Text(name(&q.part).or_view()?),
        ];
        if let Some(key) = &q.key {
            filters.push("r.key = ?".to_owned());
            args.push(Sql::Text(key.clone()));
        }
        if let Some(prefix) = &q.prefix {
            // A half-open key range: every key that starts with the prefix sorts in it.
            filters.push("r.key >= ? AND r.key < ?".to_owned());
            args.push(Sql::Text(prefix.clone()));
            args.push(Sql::Text(format!("{prefix}\u{10FFFF}")));
        }
        if q.current_only {
            filters.push("r.current = 1".to_owned());
        }
        if q.open_only {
            filters.push("r.open = 1".to_owned());
        }
        let from = match &q.index {
            Some(token) => {
                args.insert(0, Sql::Text(token.clone()));
                "task_rows r JOIN task_row_index i ON i.task = r.task AND i.part = r.part \
                 AND i.key = r.key AND i.token = ?"
            }
            None => "task_rows r",
        };
        let order = if q.newest_first { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT r.part, r.key, r.current, r.open, r.doc, r.idx FROM {from}{} ORDER BY r.key {order}",
            where_clause(&filters)
        );
        self.page(&page, &sql, args, |_, row| load::history_row(row))
    }

    fn tasks(&self, q: &TaskQuery, page: PageReq) -> Result<Page<TaskHeader>, ViewError> {
        let mut filters = Vec::new();
        let mut args: Vec<Sql> = Vec::new();
        if !q.statuses.is_empty() {
            filters.push(format!("status IN ({})", placeholders(q.statuses.len())));
            args.extend(text_args(&q.statuses));
        }
        if let Some(project) = &q.project {
            filters.push("project = ?".to_owned());
            args.push(Sql::Text(project.clone()));
        }
        let order = match q.order {
            TaskOrder::IdAsc => "num",
            TaskOrder::UpdatedDesc => "updated DESC, num",
        };
        let sql = format!(
            "SELECT {} FROM tasks{} ORDER BY {order}",
            load::HEADER_COLUMNS,
            where_clause(&filters)
        );
        self.page(&page, &sql, args, load::header)
    }

    fn events(&self, q: &EventQuery, page: PageReq) -> Result<Page<StoredEvent>, ViewError> {
        let kinds: BTreeSet<_> = q.kinds.iter().collect();
        if kinds.len() > 1 {
            // Bound each indexed kind range before merging. An IN query can otherwise
            // sort an owner's entire history before returning a small output page.
            let (offset, limit) = bounds(&page)?;
            let take = offset
                .checked_add(limit)
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| ViewError::Io(anyhow!("event page overflow")))?;
            let mut selects = Vec::new();
            let mut args = Vec::new();
            for kind in kinds {
                let mut filters = vec!["kind = ?".to_owned()];
                if q.valid_only {
                    filters.push("malformed = 0".to_owned());
                }
                args.push(Sql::Text(kind.clone()));
                if let Some(task) = &q.task {
                    filters.push("task = ?".to_owned());
                    args.push(Sql::Text(task.clone()));
                }
                if let Some(run) = &q.run {
                    filters.push("run = ?".to_owned());
                    args.push(Sql::Text(run.clone()));
                }
                if let Some(after) = q.after {
                    filters.push("seq > ?".to_owned());
                    args.push(Sql::Integer(int(after.0).or_view()?));
                }
                let order = if q.newest_first { "DESC" } else { "ASC" };
                selects.push(format!(
                    "SELECT * FROM (SELECT {} FROM events{} ORDER BY seq {order} LIMIT {take})",
                    load::EVENT_COLUMNS,
                    where_clause(&filters)
                ));
            }
            let order = if q.newest_first { "DESC" } else { "ASC" };
            let sql = format!(
                "SELECT * FROM ({}) ORDER BY seq {order}",
                selects.join(" UNION ALL ")
            );
            return self.page(&page, &sql, args, |_, row| load::event(row));
        }
        let mut filters = Vec::new();
        let mut args: Vec<Sql> = Vec::new();
        if q.valid_only {
            filters.push("malformed = 0".to_owned());
        }
        if let Some(after) = q.after {
            filters.push("seq > ?".to_owned());
            args.push(Sql::Integer(int(after.0).or_view()?));
        }
        if let Some(task) = &q.task {
            filters.push("task = ?".to_owned());
            args.push(Sql::Text(task.clone()));
        }
        if !q.kinds.is_empty() {
            filters.push(format!("kind IN ({})", placeholders(q.kinds.len())));
            args.extend(text_args(&q.kinds));
        }
        if let Some(run) = &q.run {
            filters.push("run = ?".to_owned());
            args.push(Sql::Text(run.clone()));
        }
        let order = if q.newest_first { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT {} FROM events{} ORDER BY seq {order}",
            load::EVENT_COLUMNS,
            where_clause(&filters)
        );
        self.page(&page, &sql, args, |_, row| load::event(row))
    }

    fn conditions(
        &self,
        q: &ConditionQuery,
        page: PageReq,
    ) -> Result<Page<ConditionRun>, ViewError> {
        let mut filters = Vec::new();
        let mut args: Vec<Sql> = Vec::new();
        if let Some(task) = &q.task {
            filters.push("task = ?".to_owned());
            args.push(Sql::Text(task.clone()));
        }
        if let Some(kind) = &q.kind {
            filters.push("kind = ?".to_owned());
            args.push(Sql::Text(kind.clone()));
        }
        if q.open_only {
            filters.push("closed_seq IS NULL".to_owned());
        }
        let sql = format!(
            "SELECT {} FROM condition_runs{} ORDER BY opened_seq",
            load::CONDITION_COLUMNS,
            where_clause(&filters)
        );
        self.page(&page, &sql, args, |_, row| load::condition_run(row))
    }

    fn records(&self, q: &RecordQuery, page: PageReq) -> Result<Page<StoredRecord>, ViewError> {
        let mut filters = Vec::new();
        let mut args: Vec<Sql> = Vec::new();
        if let Some(area) = &q.area {
            filters.push("area = ?".to_owned());
            args.push(Sql::Text(area.clone()));
        }
        if let Some(key) = &q.key {
            filters.push("key = ?".to_owned());
            args.push(Sql::Text(key.clone()));
        }
        let sql = format!(
            "SELECT area, key, version, doc, updated FROM records{} ORDER BY area, key",
            where_clause(&filters)
        );
        self.page(&page, &sql, args, |_, row| load::record(row))
    }

    fn aggregates(&self, names: &[AggregateName]) -> Result<Aggregates, ViewError> {
        let db = self.fresh()?;
        let mut out = Aggregates::default();
        if names.contains(&AggregateName::StatusCounts) {
            out.status_counts = load::status_counts(db).or_view()?;
        }
        if names.contains(&AggregateName::UsageTotals) {
            out.usage_totals = load::all(
                db,
                "SELECT scope, key, doc FROM usage_totals ORDER BY scope, key",
                [],
                |row| {
                    Ok(UsageTotal {
                        scope: load::text(row, 0)?,
                        key: load::text(row, 1)?,
                        doc: load::doc(row, 2)?,
                    })
                },
            )
            .or_view()?;
        }
        if names.contains(&AggregateName::PaneOwners) {
            out.pane_owners = load::all(
                db,
                "SELECT socket, n FROM pane_owners ORDER BY socket",
                [],
                |row| {
                    Ok(PaneOwner {
                        socket: load::text(row, 0)?,
                        n: load::num(row, 1)?,
                    })
                },
            )
            .or_view()?;
        }
        Ok(out)
    }

    fn blobs(&self, q: &BlobQuery, page: PageReq) -> Result<Page<BlobRow>, ViewError> {
        let mut filters = Vec::new();
        let mut args: Vec<Sql> = Vec::new();
        if let Some(hash) = &q.hash {
            filters.push("hash = ?".to_owned());
            args.push(Sql::Text(hash.as_str().to_owned()));
        }
        if let Some(state) = q.state {
            filters.push("state = ?".to_owned());
            args.push(Sql::Text(name(&state).or_view()?));
        }
        let sql = format!(
            "SELECT {} FROM blobs{} ORDER BY hash",
            load::BLOB_COLUMNS,
            where_clause(&filters)
        );
        self.page(&page, &sql, args, load::blob)
    }

    fn storage_rows(&self, q: &StorageQuery, page: PageReq) -> Result<Page<StorageRow>, ViewError> {
        match q {
            StorageQuery::Scratch { states } => {
                let names = states
                    .iter()
                    .map(name)
                    .collect::<anyhow::Result<Vec<String>>>()
                    .or_view()?;
                let filter = if names.is_empty() {
                    String::new()
                } else {
                    format!(" WHERE state IN ({})", placeholders(names.len()))
                };
                let sql = format!(
                    "SELECT {} FROM scratch_dirs{filter} ORDER BY id",
                    load::SCRATCH_COLUMNS
                );
                self.page(&page, &sql, text_args(&names).collect(), |_, row| {
                    Ok(StorageRow::Scratch(load::scratch(row)?))
                })
            }
            StorageQuery::Lifecycle { task } => {
                let (filter, args) = optional_filter("task", task);
                let sql = format!(
                    "SELECT {} FROM lifecycle{filter} ORDER BY task, generation, target",
                    load::LIFECYCLE_COLUMNS
                );
                self.page(&page, &sql, args, |_, row| {
                    Ok(StorageRow::Lifecycle(load::lifecycle(row)?))
                })
            }
            StorageQuery::Leases => self.page(
                &page,
                "SELECT id, holder, started, expires FROM leases ORDER BY id",
                Vec::new(),
                |_, row| Ok(StorageRow::Lease(load::lease(row)?)),
            ),
            StorageQuery::Claims => self.page(
                &page,
                "SELECT name, owner, acquired, doc FROM claims ORDER BY name",
                Vec::new(),
                |_, row| Ok(StorageRow::Claim(load::claim(row)?)),
            ),
            StorageQuery::Authorizations { task } => {
                let (filter, args) = optional_filter("task", task);
                let sql = format!(
                    "SELECT id, task, disposition, doc FROM authorizations{filter} ORDER BY id"
                );
                self.page(&page, &sql, args, |_, row| {
                    Ok(StorageRow::Authorization(load::authorization(row)?))
                })
            }
            StorageQuery::Authorization { id } => self.page(
                &page,
                "SELECT id, task, disposition, doc FROM authorizations WHERE id = ? ORDER BY id",
                vec![Sql::Text(id.clone())],
                |_, row| Ok(StorageRow::Authorization(load::authorization(row)?)),
            ),
        }
    }
}

fn where_clause(filters: &[String]) -> String {
    if filters.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", filters.join(" AND "))
    }
}

fn optional_filter(column: &str, value: &Option<String>) -> (String, Vec<Sql>) {
    match value {
        Some(value) => (
            format!(" WHERE {column} = ?"),
            vec![Sql::Text(value.clone())],
        ),
        None => (String::new(), Vec::new()),
    }
}
