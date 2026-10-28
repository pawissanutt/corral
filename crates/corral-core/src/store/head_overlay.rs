//! Keyed reads of a head mutation (plan T-253 section 4, "`TaskRef` and `TaskMut`"): a mutable
//! head reads through the rows it would store, so an operation sees its own queued edits. The
//! overlay holds the queued and demoted rows, the current projections and the findings they
//! derive; stored rows of an overlaid family are read whole by the query's own predicate.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;

use super::head::TaskHead;
use super::head_findings::finding_of;
use super::head_mut::TaskHeadMut;
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::{FindingSource, finding_key};

type Key = (TaskPart, String);

#[derive(Debug, Default)]
pub(crate) struct Overlay {
    rows: BTreeMap<Key, Option<HistoryRow>>,
}

impl Overlay {
    pub(crate) fn touches(&self, part: TaskPart) -> bool {
        self.rows.keys().any(|(p, _)| *p == part)
    }

    /// `stored` (every stored row matching `q`) as this mutation would leave it.
    pub(crate) fn apply(
        &self,
        q: &RowQuery,
        stored: Vec<HistoryRow>,
        limit: u32,
    ) -> Vec<HistoryRow> {
        let mut found: BTreeMap<String, HistoryRow> = stored
            .into_iter()
            .map(|row| (row.key.clone(), row))
            .collect();
        for ((part, key), row) in &self.rows {
            if *part != q.part {
                continue;
            }
            match row {
                Some(row) if q.matches(row) => {
                    found.insert(key.clone(), row.clone());
                }
                _ => {
                    found.remove(key);
                }
            }
        }
        let mut rows: Vec<HistoryRow> = found.into_values().collect();
        if q.newest_first {
            rows.reverse();
        }
        rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        rows
    }
}

fn container_ordinal(key: &str) -> Option<usize> {
    key.split('/').next()?.parse().ok()
}

impl TaskHeadMut {
    pub(crate) fn overlaid(&self) -> Result<TaskHead> {
        let mut rows: BTreeMap<Key, Option<HistoryRow>> = self.rows.clone();
        for row in self.pending_rows()? {
            rows.insert((row.part, row.key.clone()), Some(row));
        }
        let mut findings = Vec::new();
        for ((part, key), row) in &rows {
            let (source, container) = match part {
                TaskPart::ReviewRun => (FindingSource::Review, TaskPart::Run),
                TaskPart::PlanReview => (FindingSource::Plan, TaskPart::Plan),
                _ => continue,
            };
            let (Some(s), Ok(n)) = (container_ordinal(key), super::head::last_ordinal(key)) else {
                continue;
            };
            let finding = match row {
                Some(row) => {
                    let owner = self.container_id(container, s, &rows)?;
                    finding_of(*part, row, &owner)?
                }
                None => None,
            };
            let finding_row_key = finding_key(source, s, n);
            let finding_row = finding
                .map(|f| super::head_keyed::finding_row(finding_row_key.clone(), &f))
                .transpose()?;
            findings.push(((TaskPart::Finding, finding_row_key), finding_row));
        }
        rows.extend(findings);
        let mut head = self.head.clone();
        head.overlay = Some(Arc::new(Overlay { rows }));
        Ok(head)
    }

    pub(crate) fn has_pending_writes(&self) -> bool {
        !self.rows.is_empty() || !self.demoted.is_empty() || self.head != self.base
    }
}
