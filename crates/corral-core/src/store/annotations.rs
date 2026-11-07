//! Keyed annotation rows and their constant-size head facts.
use super::{TaskHead, TaskHeadMut};
use crate::annotations::*;
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Result, ensure};

pub(crate) fn thread_key(n: u32) -> String {
    format!("{n:06}")
}
pub(crate) fn entry_key(n: u32, seq: u32) -> String {
    format!("{n:06}/{seq:06}")
}
pub(crate) fn thread_row(thread: &AnnotationThread) -> Result<HistoryRow> {
    let mut metadata = thread.clone();
    metadata.entries.clear();
    Ok(HistoryRow {
        part: TaskPart::AnnotationThread,
        key: thread_key(thread.number()?),
        current: false,
        open: thread.state == AnnotationState::Open,
        doc: serde_json::to_value(metadata)?,
        index: vec![format!("id:{}", thread.id)],
    })
}
pub(crate) fn entry_row(n: u32, entry: &AnnotationEntry) -> Result<HistoryRow> {
    Ok(HistoryRow {
        part: TaskPart::AnnotationEntry,
        key: entry_key(n, entry.seq),
        current: false,
        open: false,
        doc: serde_json::to_value(entry)?,
        index: vec![
            format!("thread:N{n}"),
            format!("request:{}", entry.request.key),
        ],
    })
}
impl super::Store {
    /// Uses canonical entry attribution, not a second environment-marker classifier.
    pub fn annotation_owner_admission(&self) -> Result<()> {
        require(
            self.attributed_event(None, "annotation_admission", serde_json::Value::Null)?
                .actor
                == Some(crate::Actor::Owner),
            AnnotationCode::NotOwner,
            "annotation mutations require owner entry attribution",
        )
    }
}
impl TaskHead {
    pub fn annotation_thread(&self, id: &str) -> Result<Option<AnnotationThread>> {
        self.query(
            &RowQuery::part(TaskPart::AnnotationThread).with_index(format!("id:{id}")),
            1,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .next()
        .map(|r| {
            document_version(&r.doc)?;
            let t: AnnotationThread = serde_json::from_value(r.doc)?;
            version(t.version)?;
            version(t.anchor.version)?;
            Ok(t)
        })
        .transpose()
    }
    /// At most 256 metadata rows, never note bodies.
    pub fn annotation_threads(&self) -> Result<Vec<AnnotationThread>> {
        self.query(
            &RowQuery::part(TaskPart::AnnotationThread),
            TASK_THREADS,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .map(|r| {
            document_version(&r.doc)?;
            let t: AnnotationThread = serde_json::from_value(r.doc)?;
            version(t.version)?;
            version(t.anchor.version)?;
            Ok(t)
        })
        .collect()
    }
    /// Exactly one immutable entry, or a task-indexed retry lookup.
    pub fn annotation_entry(&self, thread: u32, seq: u32) -> Result<Option<AnnotationEntry>> {
        self.annotation_entry_query(RowQuery::key(
            TaskPart::AnnotationEntry,
            entry_key(thread, seq),
        ))
        .map(|r| r.map(|(_, e)| e))
    }
    pub fn annotation_request(&self, key: &str) -> Result<Option<(String, AnnotationEntry)>> {
        self.annotation_entry_query(
            RowQuery::part(TaskPart::AnnotationEntry).with_index(format!("request:{key}")),
        )
    }
    fn annotation_entry_query(&self, q: RowQuery) -> Result<Option<(String, AnnotationEntry)>> {
        self.query(&q, 1, Counter::HistoryRowDecodes)?
            .into_iter()
            .next()
            .map(|r| {
                let n: u32 = r.key.split('/').next().unwrap_or("").parse()?;
                document_version(&r.doc)?;
                let e: AnnotationEntry = serde_json::from_value(r.doc)?;
                version(e.version)?;
                let AnnotationAuthor::Owner { version: v, .. } = &e.author;
                version(*v)?;
                Ok((format!("N{n}"), e))
            })
            .transpose()
    }
}
impl TaskHeadMut {
    /// Append one entry and maintain the thread projection and bounded summary together.
    /// Admission/replay/revision checks happen in the service before this helper.
    pub fn append_annotation(
        &mut self,
        mut thread: AnnotationThread,
        entry: AnnotationEntry,
        new: bool,
    ) -> Result<()> {
        self.head
            .annotation_summary
            .admit(thread.revision, &entry.text, new)?;
        ensure!(
            entry.seq == thread.revision + 1,
            "annotation ordinal mismatch"
        );
        thread.revision = entry.seq;
        thread.state = entry.state;
        let n = thread.number()?;
        if new {
            self.head.annotation_summary.threads += 1;
            self.head.annotation_summary.next_thread = n + 1;
        }
        self.head.annotation_summary.entries += 1;
        self.head.annotation_summary.text_bytes += entry.text.len() as u32;
        self.put(thread_row(&thread)?);
        self.put(entry_row(n, &entry)?);
        Ok(())
    }
}
