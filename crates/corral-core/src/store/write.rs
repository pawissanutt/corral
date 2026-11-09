//! The write path of the full task family (plan T-253 sections 3 and 4): one operation per
//! write, carrying only the rows whose stored documents changed, the head and the recomputed
//! summary and facts. A write that changes nothing commits nothing.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};

use super::codec::{FACTS, Split, split};
use super::plan_scope::ScopeWrite;
use super::read::Stored;
use super::{Store, id_number};
use crate::backend::{
    ApplyError, CreateTask, HistoryRow, ImportBatch, ImportedTask, Operation, Outcome, ReadView,
    RowWrite, TaskChange, TaskHeader, TaskNum, TaskPart, UpdateForm,
};
use crate::{RecoveryRecord, Task, TaskRecord};

type Rows = BTreeMap<(TaskPart, String), HistoryRow>;

fn under<'a>(
    rows: &'a Rows,
    part: TaskPart,
    prefix: &'a str,
) -> impl Iterator<Item = &'a HistoryRow> {
    rows.range((part, prefix.to_owned())..)
        .take_while(move |((p, key), _)| *p == part && key.starts_with(prefix))
        .map(|(_, row)| row)
}

fn changed(old: &Rows, new: &Rows, part: TaskPart, prefix: &str) -> bool {
    let docs = |rows: &Rows| -> Vec<(String, serde_json::Value)> {
        under(rows, part, prefix)
            .map(|row| (row.key.clone(), row.doc.clone()))
            .collect()
    };
    docs(old) != docs(new)
}

fn facts_rev(row: Option<&HistoryRow>, field: &str) -> u64 {
    row.and_then(|row| {
        let value = row.doc.get(FACTS)?.get(field)?;
        value.as_u64().or_else(|| value.as_str()?.parse().ok())
    })
    .unwrap_or(0)
}

/// Carries each container's revision counters from its stored row, advancing one when a
/// child (or wait and decision) row's stored document changed.
pub(super) fn carry_revisions(old: &Rows, new: &mut Rows) {
    // Child containers first, so a parent compares children that already carry their counters.
    let mut containers: Vec<(TaskPart, String)> = new
        .iter()
        .filter(|(_, row)| row.doc.get(FACTS).is_some())
        .map(|(key, _)| key.clone())
        .collect();
    containers.sort_by_key(|(part, _)| {
        !matches!(
            part,
            TaskPart::ReviewRun | TaskPart::PlanReview | TaskPart::AggregateReview
        )
    });
    for (part, key) in containers {
        let children = match part {
            TaskPart::Run => Some((TaskPart::ReviewRun, format!("{key}/"))),
            TaskPart::Plan => Some((TaskPart::PlanReview, format!("{key}/"))),
            TaskPart::AggregateAttempt => Some((TaskPart::AggregateReview, format!("{key}/"))),
            _ => None,
        };
        let obligations = match part {
            TaskPart::Run => Some(format!("{key}/000000/")),
            TaskPart::ReviewRun => Some(format!("{key}/")),
            _ => None,
        };
        let stored = old.get(&(part, key.clone()));
        let mut children_rev = facts_rev(stored, "children_rev");
        if let Some((child, prefix)) = &children
            && changed(old, new, *child, prefix)
        {
            children_rev += 1;
        }
        let mut obligations_rev = facts_rev(stored, "obligations_rev");
        if let Some(prefix) = &obligations
            && changed(old, new, TaskPart::RunObligation, prefix)
        {
            obligations_rev += 1;
        }
        if let Some(facts) = new
            .get_mut(&(part, key))
            .and_then(|row| row.doc.get_mut(FACTS))
            .and_then(|facts| facts.as_object_mut())
        {
            facts.insert(
                "children_rev".to_owned(),
                format!("{children_rev:020}").into(),
            );
            facts.insert(
                "obligations_rev".to_owned(),
                format!("{obligations_rev:020}").into(),
            );
        }
    }
}

pub(super) fn row_writes(old: &Rows, new: &Rows) -> Vec<RowWrite> {
    let mut writes: Vec<RowWrite> = new
        .iter()
        .filter(|(key, row)| old.get(*key) != Some(*row))
        .map(|(_, row)| RowWrite::Put(row.clone()))
        .collect();
    writes.extend(
        old.keys()
            .filter(|key| !new.contains_key(*key))
            .map(|(part, key)| RowWrite::Delete {
                part: *part,
                key: key.clone(),
            }),
    );
    writes
}

/// The head form when every write keeps history (new or current rows, flag-only changes, or
/// deletions of current rows), else the full form.
pub(super) fn form_of(old: &Rows, writes: &[RowWrite]) -> UpdateForm {
    let head = writes.iter().all(|write| match write {
        RowWrite::Put(row) => old
            .get(&(row.part, row.key.clone()))
            .is_none_or(|stored| stored.current || stored.doc == row.doc),
        RowWrite::Delete { part, key } => old
            .get(&(*part, key.clone()))
            .is_none_or(|stored| stored.current),
    });
    if head {
        UpdateForm::Head
    } else {
        UpdateForm::Full
    }
}

pub(crate) fn apply_error(error: ApplyError) -> anyhow::Error {
    match error {
        ApplyError::Conflict(conflict) => {
            anyhow::anyhow!("concurrent write refused: {conflict:?}")
        }
        other => anyhow::anyhow!("{other}"),
    }
}

/// A task read for a read-modify-write: the revision its callback started from travels with
/// it, so [`Store::save_held`] refuses a commit that landed in between instead of replacing it.
pub(super) struct Held {
    stored: Stored,
}

impl Held {
    pub(super) fn task(&self) -> Task {
        self.stored.record.task.clone()
    }
}

impl Store {
    pub(super) fn save_inner(&self, task: &Task) -> Result<Task> {
        self.save_task(task, ScopeWrite::Preserve)
    }

    /// Reads `id` for a read-modify-write; save the changed copy with [`Store::save_held`].
    #[track_caller]
    pub(super) fn hold(&self, id: &str) -> Result<Held> {
        let id = Store::normalize_id(id)?;
        let view = self.reader()?;
        self.hold_on(&id, &*view)
    }

    /// Hydrate on the fresh lock-local view that the callback's record reads will share.
    #[track_caller]
    pub(super) fn hold_on(&self, id: &str, view: &dyn ReadView) -> Result<Held> {
        let id = Store::normalize_id(id)?;
        match self.stored_on(view, &id, std::panic::Location::caller())? {
            Some(stored) => Ok(Held { stored }),
            None => bail!("task {id} not found"),
        }
    }

    /// Saves `task` over exactly the revision `held` read; a write that landed since is a
    /// conflict and nothing is stored.
    pub(super) fn save_held(&self, held: &Held, task: &Task) -> Result<Task> {
        self.save_held_scoped(held, task, ScopeWrite::Preserve)
    }

    pub(super) fn save_held_scoped(
        &self,
        held: &Held,
        task: &Task,
        scope_write: ScopeWrite,
    ) -> Result<Task> {
        ensure!(
            Store::normalize_id(&task.id)? == held.stored.record.task.id,
            "a read-modify-write cannot change the task id {}",
            held.stored.record.task.id
        );
        self.save_over(&held.stored, task, scope_write)
    }

    pub(super) fn save_task(&self, task: &Task, scope_write: ScopeWrite) -> Result<Task> {
        let id = Store::normalize_id(&task.id)?;
        let Some(stored) = self.read_stored(&id)? else {
            if self.frozen.is_some() {
                bail!("a read-only task snapshot cannot save {}", task.id);
            }
            self.check_runtime()?;
            let mut t = task.clone();
            let now = self.now();
            t.updated = now;
            super::normalize::task(&mut t, None, now)?;
            self.insert_exact(&t)?;
            return Ok(t);
        };
        self.save_over(&stored, task, scope_write)
    }

    fn save_over(&self, stored: &Stored, task: &Task, scope_write: ScopeWrite) -> Result<Task> {
        let _unit = self.event_unit();
        if self.frozen.is_some() {
            bail!("a read-only task snapshot cannot save {}", task.id);
        }
        self.check_runtime()?;
        let before = stored.record.task.clone();
        match scope_write {
            ScopeWrite::Preserve => before.check_scope_unchanged(task)?,
            ScopeWrite::MemberGate => before.check_approved_scope_unchanged(task)?,
            ScopeWrite::Amend => {}
        }
        let decision = self.protect_owner_change(&before, task)?;
        let mut t = task.clone();
        t.path = PathBuf::new();
        let now = self.now();
        super::normalize::task(&mut t, Some(&before), now)?;
        t.updated = if t == before { before.updated } else { now };
        let record = TaskRecord {
            task: t.clone(),
            recoveries: stored.record.recoveries.clone(),
        };
        self.log_status_unpark(&before, &t)?;
        for decision in decision {
            self.log_event(
                Some(&t.id),
                "idea_decided",
                serde_json::json!({
                    "action": decision.detail["action"], "reason": decision.detail["reason"],
                    "actor": "owner", "owner_ok": true, "decision_at": decision.ts,
                    "task_updated": t.updated,
                    "into": t.idea_decision.as_ref().and_then(|d| d.into.as_deref())
                }),
            )?;
        }
        // Edit audits precede semantic events; decision proofs retain their causal order.
        let callback_events = self.buffered_events();
        let fields = crate::edited::fields(&before, &t);
        if !fields.is_empty() {
            self.log_event(Some(&t.id), "edited", serde_json::json!({"fields": fields}))?;
        }
        self.buffer_events(callback_events)?;
        if self.write_record(stored, &record, false)?.is_none() {
            return Ok(before);
        }
        Ok(t)
    }

    /// Writes `record` over `stored` as one `UpdateTask`; `None` when nothing changed, else the
    /// new revision. A change of `updated` alone counts as a change only for `exact` writes.
    pub(crate) fn write_record(
        &self,
        stored: &Stored,
        record: &TaskRecord,
        exact: bool,
    ) -> Result<Option<u64>> {
        let mut old = stored.rows();
        // Gate attempt rows are written only by the gate operations; no `Task` field encodes
        // them, so a task write neither rewrites nor deletes them.
        old.retain(|(part, _), _| *part != crate::backend::TaskPart::GateAttempt);
        let Split {
            fields,
            doc,
            summary,
            mut rows,
        } = split(record)?;
        carry_revisions(&old, &mut rows);
        super::cleanup_panes::carry_ledger(&old, &mut rows);
        let head = &stored.doc.head;
        let mut same_fields = fields.clone();
        if !exact {
            same_fields.updated = head.header.fields.updated;
        }
        let events = self.buffered_task_events(&record.task.id, record.task.updated)?;
        let records = self.buffered_records();
        let conditions = self.task_conditions(
            &record.task.id,
            stored.record.task.status,
            record.task.status,
        )?;
        if events.is_empty()
            && records.is_empty()
            && conditions.is_empty()
            && !self.has_buffered_authorization()
            && !self.has_staged_gate()
            && same_fields == head.header.fields
            && doc == head.doc
            && summary == head.summary
            && rows == old
        {
            return Ok(None);
        }
        let writes = row_writes(&old, &rows);
        let form = form_of(&old, &writes);
        let committed = self
            .db
            .backend
            .apply(self.task_operation(
                TaskChange {
                    num: head.header.num,
                    expected_rev: head.header.rev,
                    fields,
                    doc,
                    summary,
                    rows: writes,
                    events,
                    records,
                    aggregates: Vec::new(),
                    reservations: Vec::new(),
                    conditions,
                },
                form,
            )?)
            .map_err(apply_error)
            .with_context(|| format!("saving {}", record.task.id))?;
        match committed.outcome {
            Outcome::Task { rev, .. } => Ok(Some(rev)),
            other => bail!("saving {} returned {other:?}", record.task.id),
        }
    }

    /// Creates a new task with one `CreateTask`, which allocates its id.
    pub(super) fn insert_new(&self, task: &Task) -> Result<Task> {
        self.check_runtime()?;
        let mut t = task.clone();
        t.path = PathBuf::new();
        let now = self.now();
        t.updated = now;
        super::normalize::task(&mut t, None, now)?;
        let split = split(&TaskRecord {
            task: t.clone(),
            recoveries: Vec::new(),
        })?;
        self.refuse_buffered_conditions("a task creation")?;
        let committed = self
            .db
            .backend
            .apply(Operation::CreateTask(CreateTask {
                id_prefix: "T".to_owned(),
                created: t.created,
                fields: split.fields,
                doc: split.doc,
                summary: split.summary,
                rows: split.rows.into_values().collect(),
                records: self.buffered_records(),
                events: self.buffered_events(),
                reservations: Vec::new(),
            }))
            .map_err(apply_error)
            .context("creating a task")?;
        let Outcome::Created { id, .. } = committed.outcome else {
            bail!("creating a task returned {:?}", committed.outcome);
        };
        ensure!(
            id == t.id,
            "the store allocated {id} for {}; a writer bypassed the dispatch lease",
            t.id
        );
        Ok(t)
    }

    /// Stores a task under its own id and number exactly as given (a save of a task the store
    /// lacks, and fixtures).
    pub(super) fn insert_exact(&self, task: &Task) -> Result<()> {
        self.insert_batch(std::slice::from_ref(task))
    }

    /// Stores tasks under their own ids and numbers in one operation: all of them or none.
    pub(super) fn insert_batch(&self, tasks: &[Task]) -> Result<()> {
        let records: Vec<TaskRecord> = tasks
            .iter()
            .map(|task| TaskRecord {
                task: task.clone(),
                recoveries: Vec::new(),
            })
            .collect();
        self.insert_records(&records)
    }

    /// Publishes staged tasks under their own ids exactly as given, one operation each in the
    /// given order, replacing any earlier publication of the same id.
    ///
    /// # Errors
    /// Refuses snapshots and ids a fixture blocked; returns read or write errors.
    pub fn publish_staged(&self, tasks: &[Task]) -> Result<()> {
        ensure!(
            !self.is_snapshot(),
            "a read-only task snapshot cannot publish tasks"
        );
        self.check_runtime()?;
        for task in tasks {
            let id = Store::normalize_id(&task.id)?;
            ensure!(
                !self
                    .db
                    .blocked
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&id),
                "publishing {id}: blocked by a fixture"
            );
            self.fixture_put_task(task)
                .with_context(|| format!("publishing {id}"))?;
        }
        Ok(())
    }

    pub(super) fn insert_records(&self, records: &[TaskRecord]) -> Result<()> {
        let mut imported = Vec::new();
        for record in records {
            let mut t = record.task.clone();
            t.path = PathBuf::new();
            let split = split(&TaskRecord {
                task: t.clone(),
                recoveries: record.recoveries.clone(),
            })?;
            imported.push(ImportedTask {
                header: TaskHeader {
                    num: TaskNum(id_number(&t.id)),
                    id: Store::normalize_id(&t.id)?,
                    rev: 1,
                    created: t.created,
                    fields: split.fields,
                },
                doc: split.doc,
                summary: split.summary,
                rows: split.rows.into_values().collect(),
            });
        }
        self.db
            .backend
            .apply(Operation::ImportBatch(ImportBatch {
                blobs: Vec::new(),
                tasks: imported,
                events: Vec::new(),
                records: Vec::new(),
            }))
            .map_err(apply_error)
            .context("storing tasks")?;
        Ok(())
    }

    /// Reads the live canonical record, including recovery evidence.
    ///
    /// # Errors
    /// Refuses frozen snapshots; otherwise returns lookup or decoding errors.
    pub fn task_record(&self, id: &str) -> Result<TaskRecord> {
        ensure!(
            !self.is_snapshot(),
            "a read-only task snapshot cannot read live recovery records"
        );
        let id = Store::normalize_id(id)?;
        Ok(self
            .read_stored(&id)?
            .with_context(|| format!("task {id} not found"))?
            .record)
    }

    /// Durably persists intent/outcomes without evaluating, authorizing or executing recovery.
    /// An equivalent retry writes nothing.
    ///
    /// # Errors
    /// Refuses changed intent, rewritten history and invalid records; returns write errors.
    pub fn save_recovery_record(&self, task: &str, recovery: &RecoveryRecord) -> Result<()> {
        ensure!(
            !self.is_snapshot(),
            "a read-only task snapshot cannot save recovery records"
        );
        let _board = self.lock("dispatch")?;
        let task = Store::normalize_id(task)?;
        let _task = self.lock(&task)?;
        self.check_runtime()?;
        recovery.validate()?;
        ensure!(
            recovery.preview.request.target.store == self.root(),
            "recovery target does not match canonical store"
        );
        let stored = self
            .read_stored(&task)?
            .with_context(|| format!("task {task} not found"))?;
        let mut record = stored.record.clone();
        if let Some(previous) = record.recoveries.iter_mut().find(|r| r.id == recovery.id) {
            recovery.preserves(previous)?;
            if previous == recovery {
                return Ok(());
            }
            *previous = recovery.clone();
        } else {
            ensure!(
                recovery.preview.request.target.task == record.task.id,
                "recovery target does not match canonical task"
            );
            record.recoveries.push(recovery.clone());
        }
        record.task.updated = self.now();
        self.write_record(&stored, &record, false)?;
        Ok(())
    }
}
