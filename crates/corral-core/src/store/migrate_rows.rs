//! The task rows `corral migrate` imports and the record it reads back to check them: the same
//! `split` and `assemble` every write and read use, plus the cleanup-ledger rows only a legacy
//! task's creation events name.

use std::path::PathBuf;

use anyhow::Result;

use super::Store;
use super::assemble::{TaskRows, assemble};
use super::codec::{Rows, split};
use crate::backend::{ImportedTask, TaskDoc, TaskHeader, TaskNum};
use crate::{Event, TaskRecord};

/// The imported form of one legacy task; `events` are that task's legacy events.
pub(crate) fn import_task(record: &TaskRecord, events: &[Event]) -> Result<ImportedTask> {
    let mut record = record.clone();
    record.task.path = PathBuf::new();
    let split = split(&record)?;
    let mut rows = Rows(split.rows);
    super::cleanup_panes::derive_event_ledger(&mut rows, &record.task.runs, events);
    Ok(ImportedTask {
        header: TaskHeader {
            num: TaskNum(super::id_number(&record.task.id)),
            id: Store::normalize_id(&record.task.id)?,
            rev: 1,
            created: record.task.created,
            fields: split.fields,
        },
        doc: split.doc,
        summary: split.summary,
        rows: rows.0.into_values().collect(),
    })
}

pub(crate) fn assembled(doc: &TaskDoc) -> Result<TaskRecord> {
    assemble(&doc.head, &TaskRows::new(&doc.rows))
}
