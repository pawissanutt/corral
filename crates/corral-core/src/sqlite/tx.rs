//! The write path: one `BEGIN IMMEDIATE` transaction per operation. This module holds the
//! dispatch, the shared task write, events, blob references, reservations, status counts and
//! the change entry; the logic follows the reference fake so both pass one conformance suite.

use std::collections::BTreeSet;

use jiff::Timestamp;
use rusqlite::Connection;

use super::codec::*;
use super::load::{self, first};
use crate::backend::*;
use crate::blob_ref::{self, BlobHash};

pub(super) type Applied<T> = Result<T, ApplyError>;

/// The stored document that holds a set of blob references, with a structured key so distinct
/// identities never share an owner.
pub(super) enum Owner<'a> {
    TaskHead(TaskNum),
    Row(TaskNum, TaskPart, &'a str),
    Event(EventSeq),
    Record(&'a str, &'a str),
    Authorization(&'a str),
    Claim(&'a str),
    UsageTotal(&'a str, &'a str),
}

impl Owner<'_> {
    fn key(&self) -> anyhow::Result<(&'static str, String)> {
        Ok(match self {
            Self::TaskHead(num) => ("task_head", json(&[num.0])?),
            Self::Row(num, part, key) => ("row", json(&(num.0, part, key))?),
            Self::Event(seq) => ("event", json(&[seq.0])?),
            Self::Record(area, key) => ("record", json(&(area, key))?),
            Self::Authorization(id) => ("authorization", json(&[id])?),
            Self::Claim(name) => ("claim", json(&[name])?),
            Self::UsageTotal(scope, key) => ("usage_total", json(&(scope, key))?),
        })
    }
}

pub(super) struct Tx<'a> {
    pub(super) db: &'a Connection,
    pub(super) now: Timestamp,
    hooks: &'a super::Hooks,
    scopes: BTreeSet<ChangeScope>,
    events: Vec<EventSeq>,
    reservations: Vec<ReservationUse>,
}

pub(super) fn run(
    db: &Connection,
    op: Operation,
    now: Timestamp,
    hooks: &super::Hooks,
) -> Applied<Committed> {
    let kind = op.kind();
    let mut tx = Tx {
        db,
        now,
        hooks,
        scopes: BTreeSet::new(),
        events: Vec::new(),
        reservations: op_reservations(&op),
    };
    let outcome = tx.dispatch(op)?;
    tx.release_reservations()?;
    let seq = ChangeSeq(load::meta_u64(db, "change_seq").or_io()? + 1);
    db.execute(
        "INSERT INTO changes(seq, at, op) VALUES (?1, ?2, ?3)",
        rusqlite::params![int(seq.0).or_io()?, ts(now), name(&kind).or_io()?],
    )?;
    for scope in &tx.scopes {
        db.execute(
            "INSERT INTO change_scopes(seq, kind, key) VALUES (?1, ?2, ?3)",
            rusqlite::params![int(seq.0).or_io()?, scope_kind(scope), json(scope).or_io()?],
        )?;
    }
    db.execute(
        "UPDATE meta SET value = ?1 WHERE key = 'change_seq'",
        rusqlite::params![seq.0.to_string()],
    )?;
    tx.hit("change entry")?;
    Ok(Committed {
        seq,
        outcome,
        events: tx.events,
    })
}

pub(super) fn scope_kind(scope: &ChangeScope) -> &'static str {
    match scope {
        ChangeScope::Task { .. } => "task",
        ChangeScope::Record { .. } => "record",
        ChangeScope::Events { .. } => "events",
        ChangeScope::Artifact { .. } => "artifact",
        ChangeScope::Blob { .. } => "blob",
        ChangeScope::Scratch { .. } => "scratch",
        ChangeScope::Lifecycle { .. } => "lifecycle",
        ChangeScope::Condition { .. } => "condition",
        ChangeScope::Aggregate { .. } => "aggregate",
    }
}

pub(super) fn require(condition: bool, message: impl FnOnce() -> String) -> Applied<()> {
    if condition {
        Ok(())
    } else {
        Err(ApplyError::Precondition(message()))
    }
}

pub(super) fn has_event(events: &[NewEvent], kind: &str) -> bool {
    events.iter().any(|event| event.kind == kind)
}

pub(super) fn missing_task(num: TaskNum) -> ApplyError {
    ApplyError::precondition(format!("task {} does not exist", num.0))
}

fn op_reservations(op: &Operation) -> Vec<ReservationUse> {
    match op {
        Operation::CreateTask(op) => op.reservations.clone(),
        Operation::AppendEvents(op) => op.reservations.clone(),
        Operation::PutRecords(op) => op.reservations.clone(),
        Operation::Claim(op) => op.reservations.clone(),
        other => other
            .task_change()
            .map(|change| change.reservations.clone())
            .unwrap_or_default(),
    }
}

fn head_refs(doc: &serde_json::Value, summary: &serde_json::Value) -> BTreeSet<BlobHash> {
    let mut refs = blob_ref::scan(doc);
    refs.extend(blob_ref::scan(summary));
    refs
}

impl Tx<'_> {
    fn dispatch(&mut self, op: Operation) -> Applied<Outcome> {
        if let Some(change) = op.task_change() {
            self.check_rev(change)?;
        }
        match op {
            Operation::CreateTask(op) => self.create_task(op),
            Operation::UpdateTask(op) => self.write_task(&op.change, op.form, Vec::new()),
            Operation::AppendEvents(op) => {
                for event in op.events {
                    self.append_event(event)?;
                }
                Ok(Outcome::Applied)
            }
            Operation::Comment(op) => self.comment(op),
            Operation::Done(op) => self.done(op),
            Operation::Verdict(op) => self.verdict(op),
            Operation::PlanSubmit(op) => self.plan_submit(op),
            Operation::PlanVerdict(op) => self.plan_verdict(op),
            Operation::GateStart(op) => self.gate_start(op),
            Operation::GateLaunched(op) => self.gate_launched(op),
            Operation::GateResult(op) => self.gate_result(op),
            Operation::PromoteArtifacts(op) => self.promote_artifacts(op),
            Operation::PurgeArtifact(op) => self.purge_artifact(op),
            Operation::Authorization(op) => self.authorization(op),
            Operation::Claim(op) => self.claim(op),
            Operation::ReleaseClaim(op) => self.release_claim(op),
            Operation::PutRecords(op) => {
                for write in &op.writes {
                    self.put_record(write)?;
                }
                for event in op.events {
                    self.append_event(event)?;
                }
                Ok(Outcome::Applied)
            }
            Operation::BindSession(op) => self.bind_session(op),
            Operation::ObserveCondition(op) => self.observe_condition(op),
            Operation::ClearCondition(op) => self.clear_condition(op),
            Operation::Blob(op) => self.blob(op),
            Operation::Scratch(op) => self.scratch(op),
            Operation::Lifecycle(op) => self.lifecycle(op),
            Operation::Lease(op) => self.lease(op),
            Operation::ImportBatch(op) => self.import(op),
        }
    }

    pub(super) fn hit(&self, what: &str) -> Applied<()> {
        self.hooks.hit(what)
    }

    pub(super) fn scope(&mut self, scope: ChangeScope) {
        self.scopes.insert(scope);
    }

    pub(super) fn append_event(&mut self, event: NewEvent) -> Applied<EventSeq> {
        self.insert_event(event, false)
    }

    pub(super) fn insert_event(
        &mut self,
        mut event: NewEvent,
        malformed: bool,
    ) -> Applied<EventSeq> {
        event.run = event
            .detail
            .get("run")
            .and_then(|run| run.as_str())
            .map(str::to_owned);
        let last: i64 =
            self.db
                .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
                    row.get(0)
                })?;
        let seq = EventSeq(uint(last).or_io()? + 1);
        let refs = blob_ref::scan(&event.detail);
        self.scope(ChangeScope::Events {
            task: event.task.clone(),
            kind: event.kind.clone(),
        });
        self.db.execute(
            "INSERT INTO events(seq, ts, task, kind, actor, via, run, detail, malformed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                int(seq.0).or_io()?,
                ts(event.ts),
                event.task,
                event.kind,
                event.actor,
                event.via,
                event.run,
                json(&event.detail).or_io()?,
                i64::from(malformed),
            ],
        )?;
        if !malformed {
            super::lesson_approvals::put(self.db, &event).or_io()?;
        }
        self.hit("event")?;
        self.set_refs(Owner::Event(seq), refs)?;
        self.events.push(seq);
        Ok(seq)
    }

    /// Replaces the reference set of one owner, keeping blob counts and states exact.
    pub(super) fn set_refs(&mut self, owner: Owner<'_>, refs: BTreeSet<BlobHash>) -> Applied<()> {
        let (kind, key) = owner.key().or_io()?;
        let old: BTreeSet<BlobHash> = load::all(
            self.db,
            "SELECT blob FROM blob_refs WHERE owner_kind = ?1 AND owner_key = ?2",
            rusqlite::params![kind, key],
            |row| BlobHash::parse(&load::text(row, 0)?),
        )
        .or_io()?
        .into_iter()
        .collect();
        if old == refs {
            return Ok(());
        }
        for hash in refs.difference(&old) {
            let state: Option<String> = first(
                self.db,
                "SELECT state FROM blobs WHERE hash = ?1",
                rusqlite::params![hash.as_str()],
                |row| load::text(row, 0),
            )
            .or_io()?;
            let Some(state) = state else {
                return Err(ApplyError::precondition(format!(
                    "reference to unknown blob {hash}"
                )));
            };
            match from_name::<BlobState>(&state).or_io()? {
                BlobState::Live => {}
                BlobState::Writing => {
                    let mut reserved = false;
                    for used in self.reservations.iter().filter(|r| &r.hash == hash) {
                        reserved |= self.has_reservation(hash, &used.token)?;
                    }
                    require(reserved, || {
                        format!("blob {hash} is still being written; the reservation is required")
                    })?;
                }
                BlobState::Deleting => {
                    return Err(ApplyError::precondition(format!(
                        "blob {hash} is being deleted"
                    )));
                }
            }
            self.db.execute(
                "UPDATE blobs SET refs = refs + 1, state = 'live', unreferenced_since = NULL \
                 WHERE hash = ?1",
                rusqlite::params![hash.as_str()],
            )?;
            self.db.execute(
                "INSERT INTO blob_refs(owner_kind, owner_key, blob) VALUES (?1, ?2, ?3)",
                rusqlite::params![kind, key, hash.as_str()],
            )?;
            self.scope(ChangeScope::Blob { hash: hash.clone() });
            self.hit("blob ref add")?;
        }
        for hash in old.difference(&refs) {
            self.db.execute(
                "UPDATE blobs SET refs = MAX(refs - 1, 0), unreferenced_since = \
                 CASE WHEN refs <= 1 THEN ?2 ELSE unreferenced_since END WHERE hash = ?1",
                rusqlite::params![hash.as_str(), ts(self.now)],
            )?;
            self.db.execute(
                "DELETE FROM blob_refs WHERE owner_kind = ?1 AND owner_key = ?2 AND blob = ?3",
                rusqlite::params![kind, key, hash.as_str()],
            )?;
            self.scope(ChangeScope::Blob { hash: hash.clone() });
            self.hit("blob ref remove")?;
        }
        Ok(())
    }

    fn has_reservation(&self, hash: &BlobHash, token: &str) -> Applied<bool> {
        Ok(first(
            self.db,
            "SELECT 1 FROM reservations WHERE hash = ?1 AND token = ?2",
            rusqlite::params![hash.as_str(), token],
            |_| Ok(()),
        )
        .or_io()?
        .is_some())
    }

    fn release_reservations(&mut self) -> Applied<()> {
        for used in std::mem::take(&mut self.reservations) {
            let known = first(
                self.db,
                "SELECT 1 FROM blobs WHERE hash = ?1",
                rusqlite::params![used.hash.as_str()],
                |_| Ok(()),
            )
            .or_io()?;
            require(known.is_some(), || {
                format!("reservation for unknown blob {}", used.hash)
            })?;
            let removed = self.db.execute(
                "DELETE FROM reservations WHERE hash = ?1 AND token = ?2",
                rusqlite::params![used.hash.as_str(), used.token],
            )?;
            require(removed > 0, || {
                format!("no reservation {} on blob {}", used.token, used.hash)
            })?;
            self.scope(ChangeScope::Blob {
                hash: used.hash.clone(),
            });
            self.hit("reservation release")?;
        }
        Ok(())
    }

    fn adjust_status(&mut self, fields: &TaskFields, delta: i64) -> Applied<()> {
        let project = fields.project.clone().unwrap_or_default();
        let n: Option<i64> = first(
            self.db,
            "SELECT n FROM status_counts WHERE status = ?1 AND project = ?2",
            rusqlite::params![fields.status, project],
            |row| Ok(row.get(0)?),
        )
        .or_io()?;
        let next = n.unwrap_or(0) + delta;
        if next < 0 {
            return Err(ApplyError::Io(anyhow::anyhow!(
                "status count ({}, {project}) would go negative",
                fields.status
            )));
        }
        if next == 0 {
            self.db.execute(
                "DELETE FROM status_counts WHERE status = ?1 AND project = ?2",
                rusqlite::params![fields.status, project],
            )?;
        } else {
            self.db.execute(
                "INSERT OR REPLACE INTO status_counts(status, project, n) VALUES (?1, ?2, ?3)",
                rusqlite::params![fields.status, project, next],
            )?;
        }
        self.scope(ChangeScope::Aggregate {
            name: AggregateName::StatusCounts,
        });
        self.hit("status count")
    }

    pub(super) fn task_id(&self, num: TaskNum) -> Applied<String> {
        first(
            self.db,
            "SELECT id FROM tasks WHERE num = ?1",
            rusqlite::params![int(num.0).or_io()?],
            |row| load::text(row, 0),
        )
        .or_io()?
        .ok_or_else(|| missing_task(num))
    }

    pub(super) fn row(
        &self,
        num: TaskNum,
        part: TaskPart,
        key: &str,
    ) -> Applied<Option<HistoryRow>> {
        first(
            self.db,
            "SELECT part, key, current, open, doc, idx FROM task_rows \
             WHERE task = ?1 AND part = ?2 AND key = ?3",
            rusqlite::params![int(num.0).or_io()?, name(&part).or_io()?, key],
            load::history_row,
        )
        .or_io()
    }

    pub(super) fn rows_of(&self, num: TaskNum, part: TaskPart) -> Applied<Vec<HistoryRow>> {
        load::rows(self.db, num, &TaskParts::Only(vec![part])).or_io()
    }

    fn put_row(&mut self, num: TaskNum, row: HistoryRow) -> Applied<()> {
        self.maintain_artifact_count(num, row.part, &row.key, Some(&row))?;
        self.invalidate_accepted_proof(num, row.part, &row.key, Some(&row))?;
        if row.part == TaskPart::Artifact {
            super::lesson_artifacts::put(self.db, num, &row.key, Some(&row.doc)).or_io()?;
            super::artifact_reviews::put(self.db, num, &row.key, Some(&row.doc)).or_io()?;
        }
        if row.part == TaskPart::PaneRef {
            let before = self
                .row(num, row.part, &row.key)?
                .as_ref()
                .and_then(pane_socket)
                .map(str::to_owned);
            let after = pane_socket(&row).map(str::to_owned);
            if before != after {
                if let Some(socket) = before {
                    self.aggregate(&AggregateWrite::PaneOwner { socket, delta: -1 })?;
                }
                if let Some(socket) = after {
                    self.aggregate(&AggregateWrite::PaneOwner { socket, delta: 1 })?;
                }
            }
        }
        let refs = blob_ref::scan(&row.doc);
        let task = self.task_id(num)?;
        if row.part == TaskPart::Artifact {
            self.scope(ChangeScope::Artifact { task });
        }
        let task = int(num.0).or_io()?;
        let part = name(&row.part).or_io()?;
        self.delete_index_tokens(task, &part, &row.key)?;
        self.db.execute(
            "INSERT OR REPLACE INTO task_rows(task, part, key, current, open, doc, idx) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                task,
                part,
                row.key,
                i64::from(row.current),
                i64::from(row.open),
                json(&row.doc).or_io()?,
                json(&row.index).or_io()?,
            ],
        )?;
        for token in &row.index {
            self.db.execute(
                "INSERT OR IGNORE INTO task_row_index(task, part, token, key) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![task, part, token, row.key],
            )?;
        }
        self.hit("row")?;
        self.set_refs(Owner::Row(num, row.part, &row.key), refs)
    }

    fn delete_row(&mut self, num: TaskNum, part: TaskPart, key: &str) -> Applied<()> {
        self.maintain_artifact_count(num, part, key, None)?;
        self.invalidate_accepted_proof(num, part, key, None)?;
        if part == TaskPart::Artifact {
            super::lesson_artifacts::put(self.db, num, key, None).or_io()?;
            super::artifact_reviews::put(self.db, num, key, None).or_io()?;
        }
        let socket = if part == TaskPart::PaneRef {
            self.row(num, part, key)?
                .as_ref()
                .and_then(pane_socket)
                .map(str::to_owned)
        } else {
            None
        };
        self.delete_index_tokens(int(num.0).or_io()?, &name(&part).or_io()?, key)?;
        let removed = self.db.execute(
            "DELETE FROM task_rows WHERE task = ?1 AND part = ?2 AND key = ?3",
            rusqlite::params![int(num.0).or_io()?, name(&part).or_io()?, key],
        )?;
        require(removed == 1, || {
            format!("row {part:?}/{key} of task {} does not exist", num.0)
        })?;
        self.hit("row delete")?;
        if let Some(socket) = socket {
            self.aggregate(&AggregateWrite::PaneOwner { socket, delta: -1 })?;
        }
        self.set_refs(Owner::Row(num, part, key), BTreeSet::new())
    }

    fn write_edges(&self, num: TaskNum, edges: &[TaskEdge]) -> Applied<()> {
        let task = int(num.0).or_io()?;
        self.db.execute(
            "DELETE FROM task_edges WHERE task = ?1",
            rusqlite::params![task],
        )?;
        for edge in edges {
            self.db.execute(
                "INSERT OR IGNORE INTO task_edges(task, kind, other) VALUES (?1, ?2, ?3)",
                rusqlite::params![task, edge.kind, edge.other],
            )?;
        }
        Ok(())
    }

    fn create_task(&mut self, op: CreateTask) -> Applied<Outcome> {
        require(
            !op.id_prefix.is_empty() && op.id_prefix.bytes().all(|b| b.is_ascii_uppercase()),
            || format!("task id prefix {:?} must be uppercase ASCII", op.id_prefix),
        )?;
        let keys: BTreeSet<_> = op.rows.iter().map(|row| (row.part, &row.key)).collect();
        require(keys.len() == op.rows.len(), || {
            "duplicate row keys".to_owned()
        })?;
        let last: i64 =
            self.db
                .query_row("SELECT COALESCE(MAX(num), 0) FROM tasks", [], |row| {
                    row.get(0)
                })?;
        let num = TaskNum(uint(last).or_io()? + 1);
        let id = format!("{}-{:03}", op.id_prefix, num.0);
        require(!self.id_taken(&id)?, || {
            format!("task id {id} already exists")
        })?;
        let head = TaskHeadDoc {
            header: TaskHeader {
                num,
                id: id.clone(),
                rev: 1,
                created: op.created,
                fields: op.fields,
            },
            doc: op.doc,
            summary: op.summary,
        };
        self.insert_task(head, op.rows)?;
        for mut event in op.events {
            event.task.get_or_insert_with(|| id.clone());
            self.append_event(event)?;
        }
        Ok(Outcome::Created { num, id })
    }

    pub(super) fn id_taken(&self, id: &str) -> Applied<bool> {
        Ok(first(
            self.db,
            "SELECT 1 FROM tasks WHERE id = ?1",
            rusqlite::params![id],
            |_| Ok(()),
        )
        .or_io()?
        .is_some())
    }

    /// Inserts a new task head and its rows. The head document's `spec` and `body` members live
    /// in their own columns, the rest in `state`; the history summary lives in `history`.
    pub(super) fn insert_task(&mut self, head: TaskHeadDoc, rows: Vec<HistoryRow>) -> Applied<()> {
        let num = head.header.num;
        let fields = &head.header.fields;
        self.adjust_status(fields, 1)?;
        let mut parts: Vec<TaskPart> = rows.iter().map(|row| row.part).collect();
        parts.sort();
        parts.dedup();
        let columns = load::split_head(&head.doc).or_io()?;
        self.db.execute(
            "INSERT INTO tasks(num, id, rev, title, status, project, kind, size, priority, parent, \
             tracker, design, queued, created, updated, spec_rev, spec, state, body, history, \
             doc_version, dashboard_due) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
             ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, 1, ?21)",
            rusqlite::params![
                int(num.0).or_io()?,
                head.header.id,
                int(head.header.rev).or_io()?,
                fields.title,
                fields.status,
                fields.project,
                fields.kind,
                fields.size,
                fields.priority,
                fields.parent,
                i64::from(fields.tracker),
                i64::from(fields.design),
                i64::from(fields.queued),
                ts(head.header.created),
                ts(fields.updated),
                fields.spec_rev,
                columns.spec,
                columns.state,
                columns.body,
                json(&head.summary).or_io()?,
                fields.dashboard_due.map(ts),
            ],
        )?;
        self.write_edges(num, &fields.edges)?;
        self.hit("task head")?;
        self.set_refs(Owner::TaskHead(num), head_refs(&head.doc, &head.summary))?;
        for row in rows {
            self.put_row(num, row)?;
        }
        self.scope(ChangeScope::Task {
            num,
            parts,
            history: true,
            demoted: false,
        });
        Ok(())
    }

    /// The revision CAS every task-writing operation passes before any of its own predicates.
    pub(super) fn check_rev(&self, change: &TaskChange) -> Applied<()> {
        let stored = first(
            self.db,
            "SELECT id, rev FROM tasks WHERE num = ?1",
            rusqlite::params![int(change.num.0).or_io()?],
            |row| Ok((load::text(row, 0)?, load::num(row, 1)?)),
        )
        .or_io()?;
        let (id, rev) = stored.ok_or_else(|| missing_task(change.num))?;
        if rev != change.expected_rev {
            return Err(Conflict::new(format!("task {id}"), change.expected_rev, rev).into());
        }
        Ok(())
    }

    /// The shared task write: CAS on the revision, form check, rows, head, events, records,
    /// aggregates. `extra` rows come from the operation itself.
    pub(super) fn write_task(
        &mut self,
        change: &TaskChange,
        form: UpdateForm,
        extra: Vec<RowWrite>,
    ) -> Applied<Outcome> {
        let num = change.num;
        self.check_rev(change)?;
        let old = load::head_by_num(self.db, num)
            .or_io()?
            .ok_or_else(|| missing_task(num))?;
        let rev = old.header.rev;
        if form == UpdateForm::Head {
            for write in &change.rows {
                match write {
                    RowWrite::Delete { part, key } => {
                        let existing = self.row(num, *part, key)?;
                        require(existing.is_none_or(|old| old.current), || {
                            format!("head-form write may not delete history row {part:?}/{key}")
                        })?;
                    }
                    RowWrite::Put(row) => {
                        let existing = self.row(num, row.part, &row.key)?;
                        require(
                            existing.is_none_or(|old| old.current || old.doc == row.doc),
                            || {
                                format!(
                                    "head-form write may not edit history row {:?}/{}",
                                    row.part, row.key
                                )
                            },
                        )?;
                    }
                }
            }
        }
        let old_fields = old.header.fields;
        let mut parts = BTreeSet::new();
        let mut history = false;
        let mut demoted = false;
        for write in change.rows.iter().cloned().chain(extra) {
            let stored = match &write {
                RowWrite::Put(row) => self.row(num, row.part, &row.key)?,
                RowWrite::Delete { part, key } => self.row(num, *part, key)?,
            };
            history |= write.container_history(stored.as_ref());
            demoted |= write.container_demotion(stored.as_ref());
            match write {
                RowWrite::Put(row) => {
                    parts.insert(row.part);
                    self.put_row(num, row)?;
                }
                RowWrite::Delete { part, key } => {
                    parts.insert(part);
                    self.delete_row(num, part, &key)?;
                }
            }
        }
        let fields = &change.fields;
        if old_fields.status != fields.status || old_fields.project != fields.project {
            self.adjust_status(&old_fields, -1)?;
            self.adjust_status(fields, 1)?;
        }
        let columns = load::split_head(&change.doc).or_io()?;
        self.db.execute(
            "UPDATE tasks SET rev = ?2, title = ?3, status = ?4, project = ?5, priority = ?6, \
             parent = ?7, updated = ?8, state = ?9, history = ?10, kind = ?11, size = ?12, \
             tracker = ?13, design = ?14, queued = ?15, spec_rev = ?16, spec = ?17, body = ?18, \
             dashboard_due = ?19 WHERE num = ?1",
            rusqlite::params![
                int(num.0).or_io()?,
                int(rev + 1).or_io()?,
                fields.title,
                fields.status,
                fields.project,
                fields.priority,
                fields.parent,
                ts(fields.updated),
                columns.state,
                json(&change.summary).or_io()?,
                fields.kind,
                fields.size,
                i64::from(fields.tracker),
                i64::from(fields.design),
                i64::from(fields.queued),
                fields.spec_rev,
                columns.spec,
                columns.body,
                fields.dashboard_due.map(ts),
            ],
        )?;
        self.write_edges(num, &fields.edges)?;
        self.hit("task head")?;
        self.set_refs(
            Owner::TaskHead(num),
            head_refs(&change.doc, &change.summary),
        )?;
        self.scope(ChangeScope::Task {
            num,
            parts: parts.into_iter().collect(),
            history,
            demoted,
        });
        for event in &change.events {
            self.append_event(event.clone())?;
        }
        for condition in &change.conditions {
            match condition.clone() {
                ConditionWrite::Observe(op) => self.observe_condition(op)?,
                ConditionWrite::Clear(op) => self.clear_condition(op)?,
            };
        }
        for write in &change.records {
            self.put_record(write)?;
        }
        for write in &change.aggregates {
            self.aggregate(write)?;
        }
        Ok(Outcome::Task { num, rev: rev + 1 })
    }

    fn aggregate(&mut self, write: &AggregateWrite) -> Applied<()> {
        match write {
            AggregateWrite::UsageTotal { scope, key, doc } => {
                self.db.execute(
                    "INSERT OR REPLACE INTO usage_totals(scope, key, doc) VALUES (?1, ?2, ?3)",
                    rusqlite::params![scope, key, json(doc).or_io()?],
                )?;
                self.scope(ChangeScope::Aggregate {
                    name: AggregateName::UsageTotals,
                });
                self.hit("aggregate")?;
                self.set_refs(Owner::UsageTotal(scope, key), blob_ref::scan(doc))
            }
            AggregateWrite::PaneOwner { socket, delta } => {
                let n: Option<i64> = first(
                    self.db,
                    "SELECT n FROM pane_owners WHERE socket = ?1",
                    rusqlite::params![socket],
                    |row| Ok(row.get(0)?),
                )
                .or_io()?;
                let next = n.unwrap_or(0) + delta;
                require(next >= 0, || {
                    format!("pane owner count of {socket:?} would go negative")
                })?;
                if next == 0 {
                    self.db.execute(
                        "DELETE FROM pane_owners WHERE socket = ?1",
                        rusqlite::params![socket],
                    )?;
                } else {
                    self.db.execute(
                        "INSERT OR REPLACE INTO pane_owners(socket, n) VALUES (?1, ?2)",
                        rusqlite::params![socket, next],
                    )?;
                }
                self.scope(ChangeScope::Aggregate {
                    name: AggregateName::PaneOwners,
                });
                self.hit("aggregate")
            }
        }
    }
}

fn pane_socket(row: &HistoryRow) -> Option<&str> {
    (row.part == TaskPart::PaneRef).then(|| {
        row.doc
            .get("socket")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    })
}
