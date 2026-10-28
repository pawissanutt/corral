//! Full reads of the task family (plan T-253 section 4, "Sweep of the kept facade family"):
//! each assembles whole tasks from one view, counts one hydration per task inside a hot scope
//! and every decoded document by its class.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::panic::Location;
use std::sync::{Arc, MutexGuard, PoisonError};

use anyhow::Result;
use serde_json::Value;

use super::assemble::{TaskRows, assemble};
use super::{Store, id_number};
use crate::backend::{
    HistoryRow, PageReq, ReadView, TaskDoc, TaskNum, TaskOrder, TaskPart, TaskParts, TaskQuery,
};
use crate::work::{self, Counter, Subject};
use crate::{Task, TaskRecord};

const PAGE: u32 = 500;

/// One view: a fresh one per call on a live store, the pinned one on a snapshot.
pub(crate) enum Reader<'a> {
    Fresh(Box<dyn ReadView + Send>),
    Pinned(MutexGuard<'a, Box<dyn ReadView + Send>>),
}

impl Deref for Reader<'_> {
    type Target = dyn ReadView + Send;

    fn deref(&self) -> &Self::Target {
        match self {
            Reader::Fresh(view) => view.as_ref(),
            Reader::Pinned(view) => view.as_ref(),
        }
    }
}

/// A task as stored: its document and rows, and the record they assemble into.
pub(crate) struct Stored {
    pub(crate) doc: TaskDoc,
    pub(crate) record: TaskRecord,
}

impl Stored {
    pub(crate) fn rows(&self) -> BTreeMap<(TaskPart, String), HistoryRow> {
        self.doc
            .rows
            .iter()
            .map(|row| ((row.part, row.key.clone()), row.clone()))
            .collect()
    }
}

pub(crate) fn doc_bytes(doc: &Value) -> u64 {
    serde_json::to_string(doc).map_or(0, |text| u64::try_from(text.len()).unwrap_or(u64::MAX))
}

/// The run id of each coding run seq, for attributing nested rows to their run.
pub(crate) fn run_seq_of(row: &HistoryRow) -> Option<&str> {
    let key = row.key.strip_prefix("run:").unwrap_or(&row.key);
    match row.part {
        TaskPart::ReviewRun
        | TaskPart::RunUsage
        | TaskPart::RunCheckpoint
        | TaskPart::RunSwitch
        | TaskPart::RunObligation
        | TaskPart::AgentRecord => key.split('/').next(),
        TaskPart::InboxEffect | TaskPart::ObserveCursor if row.key.starts_with("run:") => {
            key.split('/').next()
        }
        _ => None,
    }
}

/// Assembles one stored task: the single place a full hydration happens, so it is counted
/// here exactly once, at the caller's site.
fn hydrate(doc: &TaskDoc, location: &'static Location<'static>) -> Result<TaskRecord> {
    work::hydrated_at(location);
    count_task_doc(doc);
    assemble(&doc.head, &TaskRows::new(&doc.rows))
}

fn count_task_doc(doc: &TaskDoc) {
    if !work::in_hot_scope() {
        return;
    }
    work::decoded(
        Counter::TaskHeadDecodes,
        doc_bytes(&doc.head.doc) + doc_bytes(&doc.head.summary),
        Subject::Plain,
    );
    for row in &doc.rows {
        let bytes = doc_bytes(&row.doc);
        let counter = match row.part {
            TaskPart::Run if row.current => Counter::RunDecodes,
            TaskPart::PlanRun | TaskPart::Plan | TaskPart::AggregateAttempt if row.current => {
                Counter::TaskHeadDecodes
            }
            TaskPart::Run
            | TaskPart::PlanRun
            | TaskPart::Plan
            | TaskPart::AggregateAttempt
            | TaskPart::Comment
            | TaskPart::Artifact
            | TaskPart::ScopeRevision
            | TaskPart::PmReceipt
            | TaskPart::MemberGateHistory
            | TaskPart::Recovery
            | TaskPart::RecoveryOutcome
            | TaskPart::PaneRef
            | TaskPart::Finding
            | TaskPart::Prompt
            | TaskPart::GateAttempt
            | TaskPart::ReviewEvidence => Counter::HistoryRowDecodes,
            _ => Counter::NestedRowDecodes,
        };
        work::decoded(counter, bytes, Subject::Plain);
    }
}

impl Store {
    pub(crate) fn reader(&self) -> Result<Reader<'_>> {
        if let Some(frozen) = &self.frozen {
            return Ok(Reader::Pinned(
                frozen.view.lock().unwrap_or_else(PoisonError::into_inner),
            ));
        }
        Ok(Reader::Fresh(self.view()?))
    }

    pub(crate) fn stored_on(
        &self,
        view: &dyn ReadView,
        id: &str,
        location: &'static Location<'static>,
    ) -> Result<Option<Stored>> {
        let num = TaskNum(id_number(id));
        let Some(doc) = view.task(num, &TaskParts::All)? else {
            return Ok(None);
        };
        if doc.head.header.id != id {
            return Ok(None);
        }
        let record = hydrate(&doc, location)?;
        Ok(Some(Stored { doc, record }))
    }

    #[track_caller]
    pub(crate) fn read_stored(&self, id: &str) -> Result<Option<Stored>> {
        self.read_stored_at(id, Location::caller())
    }

    pub(crate) fn read_stored_at(
        &self,
        id: &str,
        location: &'static Location<'static>,
    ) -> Result<Option<Stored>> {
        let reader = self.reader()?;
        self.stored_on(&*reader, id, location)
    }

    pub(crate) fn read_task(
        &self,
        id: &str,
        location: &'static Location<'static>,
    ) -> Result<Option<TaskRecord>> {
        Ok(self
            .read_stored_at(id, location)?
            .map(|stored| stored.record))
    }

    /// Every task's live canonical record, recovery evidence included, all read on one view.
    ///
    /// # Errors
    /// Refuses frozen snapshots, like [`Store::task_record`]; otherwise returns read or
    /// decoding errors.
    #[track_caller]
    pub fn task_records(&self) -> Result<Vec<TaskRecord>> {
        anyhow::ensure!(
            !self.is_snapshot(),
            "a read-only task snapshot cannot read live recovery records"
        );
        let location = Location::caller();
        let reader = self.reader()?;
        let mut records = Vec::new();
        for header in Self::headers_on(&*reader)? {
            if let Some(stored) = self.stored_on(&*reader, &header.id, location)? {
                records.push(stored.record);
            }
        }
        Ok(records)
    }

    pub(crate) fn headers_on(view: &dyn ReadView) -> Result<Vec<crate::backend::TaskHeader>> {
        let query = TaskQuery {
            statuses: Vec::new(),
            project: None,
            order: TaskOrder::IdAsc,
        };
        let mut page = PageReq::first(PAGE);
        let mut headers = Vec::new();
        loop {
            let found = view.tasks(&query, page.clone())?;
            headers.extend(found.items);
            match found.next {
                Some(next) => page.after = Some(next),
                None => return Ok(headers),
            }
        }
    }

    /// One past the highest stored task number, read without enumerating the board.
    pub fn next_task_number(&self) -> Result<u64> {
        let reader = self.reader()?;
        let last = reader.last_task_num()?.map_or(0, |num| num.0);
        anyhow::Context::context(last.checked_add(1), "task ID exhausted")
    }

    /// Whether a stored task, readable or not as a card, holds task number `num`.
    pub fn task_number_taken(&self, num: u64) -> Result<bool> {
        Ok(self.reader()?.head(TaskNum(num))?.is_some())
    }

    fn list_on(
        &self,
        view: &dyn ReadView,
        location: &'static Location<'static>,
    ) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        for header in Self::headers_on(view)? {
            let Some(doc) = view.task(header.num, &TaskParts::All)? else {
                continue;
            };
            match hydrate(&doc, location) {
                Ok(record) => tasks.push(record.task),
                Err(error) => crate::messages::report(
                    Some(&header.id),
                    "store",
                    format!("skipping {}: {error:#}", header.id),
                ),
            }
        }
        tasks.sort_by_key(|t| (id_number(&t.id), t.id.clone()));
        Ok(tasks)
    }

    pub(crate) fn read_listing(
        &self,
        location: &'static Location<'static>,
    ) -> Result<Arc<Vec<Task>>> {
        let hot = work::in_hot_scope();
        if let Some(frozen) = &self.frozen {
            let mut listing = frozen
                .listing
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(tasks) = listing.as_ref().filter(|_| !hot) {
                frozen
                    .view
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .ensure_fresh()?;
                return Ok(Arc::clone(tasks));
            }
            let view = frozen.view.lock().unwrap_or_else(PoisonError::into_inner);
            let tasks = Arc::new(self.list_on(view.as_ref(), location)?);
            if listing.is_none() {
                *listing = Some(Arc::clone(&tasks));
            }
            return Ok(listing.as_ref().map_or(tasks, Arc::clone));
        }
        let reader = self.reader()?;
        let seq = reader.seq();
        if !hot
            && let Some((cached, tasks)) = self
                .db
                .listing
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            && *cached == seq
        {
            return Ok(Arc::clone(tasks));
        }
        let tasks = Arc::new(self.list_on(&*reader, location)?);
        *self
            .db
            .listing
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((seq, Arc::clone(&tasks)));
        Ok(tasks)
    }
}
