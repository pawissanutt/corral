//! The fake's write path: one working copy per operation, a fault point after every primitive
//! write, and the task-writing operations.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use jiff::Timestamp;
use serde_json::Value;

use super::{RefOwner, State, TaskState};
use crate::backend::*;
use crate::blob_ref::{self, BlobHash};

pub(super) type Applied<T> = Result<T, ApplyError>;

pub(super) struct Tx<'a> {
    pub(super) state: State,
    pub(super) now: Timestamp,
    faults: &'a FaultInjector,
    scopes: BTreeSet<ChangeScope>,
    events: Vec<EventSeq>,
    reservations: Vec<ReservationUse>,
}

pub(super) fn run(
    state: State,
    op: Operation,
    now: Timestamp,
    faults: &FaultInjector,
) -> Result<(State, Committed), ApplyError> {
    let kind = op.kind();
    let mut tx = Tx {
        state,
        now,
        faults,
        scopes: BTreeSet::new(),
        events: Vec::new(),
        reservations: op_reservations(&op),
    };
    let outcome = tx.dispatch(op)?;
    tx.release_reservations()?;
    let seq = ChangeSeq(tx.state.seq.0 + 1);
    tx.state.seq = seq;
    let entry = ChangeEntry {
        seq,
        at: now,
        op: kind,
        scopes: tx.scopes.into_iter().collect(),
    };
    Arc::make_mut(&mut tx.state.feed).push(Arc::new(entry));
    faults.after_write("change entry")?;
    Ok((
        tx.state,
        Committed {
            seq,
            outcome,
            events: tx.events,
        },
    ))
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

pub(super) fn to_doc<T: serde::Serialize>(value: &T) -> Applied<Value> {
    serde_json::to_value(value).map_err(|error| ApplyError::Io(error.into()))
}

pub(super) fn from_doc<T: serde::de::DeserializeOwned>(value: &Value) -> Applied<T> {
    serde_json::from_value(value.clone()).map_err(|error| ApplyError::Io(error.into()))
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
        self.faults.after_write(what)
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
        let seq = EventSeq(self.state.events.last().map_or(0, |last| last.seq.0) + 1);
        let refs = blob_ref::scan(&event.detail);
        self.scope(ChangeScope::Events {
            task: event.task.clone(),
            kind: event.kind.clone(),
        });
        if !malformed && let Some((sha, row)) = super::super::lesson_approvals::attribution(&event)
        {
            Arc::make_mut(&mut self.state.lesson_approvals)
                .entry(sha.to_owned())
                .or_default()
                .insert(row);
        }
        Arc::make_mut(&mut self.state.events).push(Arc::new(StoredEvent {
            seq,
            event,
            malformed,
        }));
        self.hit("event")?;
        self.set_refs(RefOwner::Event(seq), refs)?;
        self.events.push(seq);
        Ok(seq)
    }

    /// Replaces the reference set of one owner, keeping blob counts and states exact.
    pub(super) fn set_refs(&mut self, owner: RefOwner, refs: BTreeSet<BlobHash>) -> Applied<()> {
        let old = self.state.refs.get(&owner).cloned().unwrap_or_default();
        if old == refs {
            return Ok(());
        }
        for hash in refs.difference(&old) {
            let row = self.state.blobs.get(hash).ok_or_else(|| {
                ApplyError::precondition(format!("reference to unknown blob {hash}"))
            })?;
            match row.state {
                BlobState::Live => {}
                BlobState::Writing => require(
                    self.reservations.iter().any(|r| {
                        &r.hash == hash && row.reservations.iter().any(|h| h.token == r.token)
                    }),
                    || format!("blob {hash} is still being written; the reservation is required"),
                )?,
                BlobState::Deleting => {
                    return Err(ApplyError::precondition(format!(
                        "blob {hash} is being deleted"
                    )));
                }
            }
            let blobs = Arc::make_mut(&mut self.state.blobs);
            if let Some(row) = blobs.get_mut(hash) {
                row.refs += 1;
                row.state = BlobState::Live;
                row.unreferenced_since = None;
            }
            self.scope(ChangeScope::Blob { hash: hash.clone() });
            self.hit("blob ref add")?;
        }
        for hash in old.difference(&refs) {
            let now = self.now;
            let blobs = Arc::make_mut(&mut self.state.blobs);
            if let Some(row) = blobs.get_mut(hash) {
                row.refs = row.refs.saturating_sub(1);
                if row.refs == 0 {
                    row.unreferenced_since = Some(now);
                }
            }
            self.scope(ChangeScope::Blob { hash: hash.clone() });
            self.hit("blob ref remove")?;
        }
        let table = Arc::make_mut(&mut self.state.refs);
        if refs.is_empty() {
            table.remove(&owner);
        } else {
            table.insert(owner, refs);
        }
        Ok(())
    }

    fn release_reservations(&mut self) -> Applied<()> {
        for used in std::mem::take(&mut self.reservations) {
            let blobs = Arc::make_mut(&mut self.state.blobs);
            let row = blobs.get_mut(&used.hash).ok_or_else(|| {
                ApplyError::precondition(format!("reservation for unknown blob {}", used.hash))
            })?;
            let before = row.reservations.len();
            row.reservations.retain(|r| r.token != used.token);
            require(row.reservations.len() < before, || {
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
        let key = (
            fields.status.clone(),
            fields.project.clone().unwrap_or_default(),
        );
        let counts = Arc::make_mut(&mut self.state.status_counts);
        let n = counts.get(&key).copied().unwrap_or(0);
        let next = n.checked_add_signed(delta).ok_or_else(|| {
            ApplyError::Io(anyhow::anyhow!("status count {key:?} would go negative"))
        })?;
        if next == 0 {
            counts.remove(&key);
        } else {
            counts.insert(key, next);
        }
        self.scope(ChangeScope::Aggregate {
            name: AggregateName::StatusCounts,
        });
        self.hit("status count")
    }

    fn put_row(&mut self, num: TaskNum, row: HistoryRow) -> Applied<()> {
        self.maintain_artifact_count(num, row.part, &row.key, Some(&row))?;
        self.invalidate_accepted_proof(num, row.part, &row.key, Some(&row))?;
        if row.part == TaskPart::Artifact {
            Arc::make_mut(&mut self.state.lesson_artifacts)
                .put(num, &row.key, Some(&row.doc))
                .map_err(ApplyError::Io)?;
            let old = self
                .row(num, row.part, &row.key)
                .map(|r| super::super::query::artifact_reviews::kept_path(&r.doc))
                .transpose()
                .map_err(ApplyError::Io)?
                .flatten();
            let new = super::super::query::artifact_reviews::kept_path(&row.doc)
                .map_err(ApplyError::Io)?;
            let reviews = Arc::make_mut(&mut self.state.artifact_reviews);
            if let Some(path) = old {
                reviews.remove(num, &row.key, &path);
            }
            if let Some(path) = new {
                reviews.insert(num, &row.key, path);
            }
        }
        if row.part == TaskPart::PaneRef {
            let before = self
                .row(num, row.part, &row.key)
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
        let owner = row_owner(num, row.part, &row.key);
        let refs = blob_ref::scan(&row.doc);
        if row.part == TaskPart::Artifact {
            self.scope(ChangeScope::Artifact {
                task: self.task_id(num)?,
            });
        }
        let tasks = Arc::make_mut(&mut self.state.tasks);
        let task = tasks.get_mut(&num).ok_or_else(|| missing_task(num))?;
        task.rows.insert((row.part, row.key.clone()), row);
        self.hit("row")?;
        self.set_refs(owner, refs)
    }

    fn delete_row(&mut self, num: TaskNum, part: TaskPart, key: &str) -> Applied<()> {
        self.maintain_artifact_count(num, part, key, None)?;
        self.invalidate_accepted_proof(num, part, key, None)?;
        if part == TaskPart::Artifact {
            Arc::make_mut(&mut self.state.lesson_artifacts)
                .put(num, key, None)
                .map_err(ApplyError::Io)?;
        }
        if part == TaskPart::Artifact
            && let Some(row) = self.row(num, part, key)
            && let Some(path) = super::super::query::artifact_reviews::kept_path(&row.doc)
                .map_err(ApplyError::Io)?
        {
            Arc::make_mut(&mut self.state.artifact_reviews).remove(num, key, &path);
        }
        let socket = if part == TaskPart::PaneRef {
            self.row(num, part, key)
                .and_then(pane_socket)
                .map(str::to_owned)
        } else {
            None
        };
        let tasks = Arc::make_mut(&mut self.state.tasks);
        let task = tasks.get_mut(&num).ok_or_else(|| missing_task(num))?;
        require(task.rows.remove(&(part, key.to_owned())).is_some(), || {
            format!("row {part:?}/{key} of task {} does not exist", num.0)
        })?;
        self.hit("row delete")?;
        if let Some(socket) = socket {
            self.aggregate(&AggregateWrite::PaneOwner { socket, delta: -1 })?;
        }
        self.set_refs(row_owner(num, part, key), BTreeSet::new())
    }

    pub(super) fn task_id(&self, num: TaskNum) -> Applied<String> {
        self.state
            .tasks
            .get(&num)
            .map(|task| task.head.header.id.clone())
            .ok_or_else(|| missing_task(num))
    }

    pub(super) fn row(&self, num: TaskNum, part: TaskPart, key: &str) -> Option<&HistoryRow> {
        self.state
            .tasks
            .get(&num)?
            .rows
            .get(&(part, key.to_owned()))
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
        let num = TaskNum(self.state.tasks.keys().next_back().map_or(0, |last| last.0) + 1);
        let id = format!("{}-{:03}", op.id_prefix, num.0);
        require(
            !self
                .state
                .tasks
                .values()
                .any(|task| task.head.header.id == id),
            || format!("task id {id} already exists"),
        )?;
        let head = TaskHeadDoc {
            header: TaskHeader {
                num,
                id: id.clone(),
                rev: 1,
                created: op.created,
                fields: op.fields.clone(),
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

    pub(super) fn insert_task(&mut self, head: TaskHeadDoc, rows: Vec<HistoryRow>) -> Applied<()> {
        let num = head.header.num;
        let head_refs = head_refs(&head);
        self.adjust_status(&head.header.fields, 1)?;
        let mut parts: Vec<TaskPart> = rows.iter().map(|row| row.part).collect();
        parts.sort();
        parts.dedup();
        Arc::make_mut(&mut self.state.tasks).insert(
            num,
            TaskState {
                head,
                rows: BTreeMap::new(),
            },
        );
        self.hit("task head")?;
        self.set_refs(RefOwner::TaskHead(num), head_refs)?;
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

    /// The revision CAS every task-writing operation passes before any of its own predicates,
    /// so a stale writer always gets `Conflict` whatever the competing write changed.
    pub(super) fn check_rev(&self, change: &TaskChange) -> Applied<()> {
        let task = self
            .state
            .tasks
            .get(&change.num)
            .ok_or_else(|| missing_task(change.num))?;
        let rev = task.head.header.rev;
        if rev != change.expected_rev {
            return Err(Conflict::new(
                format!("task {}", task.head.header.id),
                change.expected_rev,
                rev,
            )
            .into());
        }
        Ok(())
    }

    /// The shared task write: CAS on the revision, form check, rows, head, events, records,
    /// aggregates and reservations. `extra` rows come from the operation itself.
    pub(super) fn write_task(
        &mut self,
        change: &TaskChange,
        form: UpdateForm,
        extra: Vec<RowWrite>,
    ) -> Applied<Outcome> {
        let num = change.num;
        self.check_rev(change)?;
        let task = self
            .state
            .tasks
            .get(&num)
            .ok_or_else(|| missing_task(num))?;
        let rev = task.head.header.rev;
        if form == UpdateForm::Head {
            for write in &change.rows {
                match write {
                    RowWrite::Delete { part, key } => {
                        let existing = task.rows.get(&(*part, key.clone()));
                        require(existing.is_none_or(|old| old.current), || {
                            format!("head-form write may not delete history row {part:?}/{key}")
                        })?;
                    }
                    RowWrite::Put(row) => {
                        let existing = task.rows.get(&(row.part, row.key.clone()));
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
        let old_fields = task.head.header.fields.clone();
        let mut parts = BTreeSet::new();
        let mut history = false;
        let mut demoted = false;
        for write in change.rows.iter().cloned().chain(extra) {
            let stored = match &write {
                RowWrite::Put(row) => self.row(num, row.part, &row.key).cloned(),
                RowWrite::Delete { part, key } => self.row(num, *part, key).cloned(),
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
        if old_fields.status != change.fields.status || old_fields.project != change.fields.project
        {
            self.adjust_status(&old_fields, -1)?;
            self.adjust_status(&change.fields, 1)?;
        }
        let tasks = Arc::make_mut(&mut self.state.tasks);
        let task = tasks.get_mut(&num).ok_or_else(|| missing_task(num))?;
        task.head.header.rev = rev + 1;
        task.head.header.fields = change.fields.clone();
        task.head.doc = change.doc.clone();
        task.head.summary = change.summary.clone();
        let refs = head_refs(&task.head);
        self.hit("task head")?;
        self.set_refs(RefOwner::TaskHead(num), refs)?;
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
                Arc::make_mut(&mut self.state.usage_totals)
                    .insert((scope.clone(), key.clone()), doc.clone());
                self.scope(ChangeScope::Aggregate {
                    name: AggregateName::UsageTotals,
                });
                self.hit("aggregate")?;
                let owner = RefOwner::UsageTotal(scope.clone(), key.clone());
                return self.set_refs(owner, blob_ref::scan(doc));
            }
            AggregateWrite::PaneOwner { socket, delta } => {
                let owners = Arc::make_mut(&mut self.state.pane_owners);
                let n = owners.get(socket).copied().unwrap_or(0);
                let next = n.checked_add_signed(*delta).ok_or_else(|| {
                    ApplyError::precondition(format!(
                        "pane owner count of {socket:?} would go negative"
                    ))
                })?;
                if next == 0 {
                    owners.remove(socket);
                } else {
                    owners.insert(socket.clone(), next);
                }
                self.scope(ChangeScope::Aggregate {
                    name: AggregateName::PaneOwners,
                });
            }
        }
        self.hit("aggregate")
    }
}

fn pane_socket(row: &HistoryRow) -> Option<&str> {
    (row.part == TaskPart::PaneRef)
        .then(|| row.doc.get("socket").and_then(Value::as_str).unwrap_or(""))
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

pub(super) fn missing_task(num: TaskNum) -> ApplyError {
    ApplyError::precondition(format!("task {} does not exist", num.0))
}

pub(super) fn row_owner(num: TaskNum, part: TaskPart, key: &str) -> RefOwner {
    RefOwner::Row(num, part, key.to_owned())
}

fn head_refs(head: &TaskHeadDoc) -> BTreeSet<BlobHash> {
    let mut refs = blob_ref::scan(&head.doc);
    refs.extend(blob_ref::scan(&head.summary));
    refs
}
