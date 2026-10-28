//! Keyed reads of a head mutation (plan T-253 section 4, "`TaskRef` and `TaskMut`"): a mutable
//! head reads through the rows it would store, so an operation sees its own queued edits. The
//! overlay holds the rows this mutation changed (queued, demoted and edited current rows) and
//! the findings they derive. A read of an overlaid family pages stored rows at the requested
//! size and fetches further only for the rows the overlay displaced from a page, so its bound
//! stays the request's, not the history's.

use std::collections::BTreeMap;
use std::iter::Peekable;
use std::sync::Arc;

use anyhow::Result;

use super::head::{CheckpointMetas, TaskHead};
use super::head_findings::finding_of;
use super::head_mut::TaskHeadMut;
use crate::backend::{HistoryRow, Page, PageReq, RowQuery, TaskPart};
use crate::{CheckpointMeta, FindingSource, finding_key};

type Key = (TaskPart, String);

#[derive(Debug, Default)]
pub(crate) struct Overlay {
    rows: BTreeMap<Key, Option<HistoryRow>>,
}

impl Overlay {
    pub(crate) fn touches(&self, part: TaskPart) -> bool {
        self.rows.keys().any(|(p, _)| *p == part)
    }

    /// The first `limit` rows matching `q` as this mutation would leave them, reading stored
    /// pages with `read` until the rows the overlay does not displace fill `limit`.
    pub(crate) fn read(
        &self,
        q: &RowQuery,
        limit: u32,
        mut read: impl FnMut(PageReq) -> Result<Page<HistoryRow>>,
    ) -> Result<Vec<HistoryRow>> {
        let mut stored = Vec::new();
        let mut page = PageReq::first(limit);
        loop {
            let got = read(page)?;
            stored.extend(
                got.items
                    .into_iter()
                    .filter(|row| !self.holds(q.part, &row.key)),
            );
            let short = usize::try_from(limit)
                .unwrap_or(usize::MAX)
                .saturating_sub(stored.len());
            match got.next {
                Some(after) if short > 0 => {
                    page = PageReq {
                        after: Some(after),
                        limit: u32::try_from(short).unwrap_or(u32::MAX),
                    };
                }
                _ => break,
            }
        }
        Ok(self.apply(q, stored, "", limit))
    }

    /// Whether the stored row at `key` is replaced or removed by this overlay.
    pub(crate) fn holds(&self, part: TaskPart, key: &str) -> bool {
        self.rows.contains_key(&(part, key.to_owned()))
    }

    /// The overlay rows matching `q`, in its order.
    fn matching(&self, q: &RowQuery) -> Vec<HistoryRow> {
        let mut rows: Vec<HistoryRow> = self
            .rows
            .values()
            .flatten()
            .filter(|row| q.matches(row))
            .cloned()
            .collect();
        if q.newest_first {
            rows.reverse();
        }
        rows
    }

    /// The first `limit` rows matching `q` but the one at `except`, as this mutation would
    /// leave them. `stored` holds every stored match up to the `limit`-th one the overlay does
    /// not displace.
    pub(crate) fn apply(
        &self,
        q: &RowQuery,
        stored: Vec<HistoryRow>,
        except: &str,
        limit: u32,
    ) -> Vec<HistoryRow> {
        let mut found: BTreeMap<String, HistoryRow> = stored
            .into_iter()
            .filter(|row| !self.holds(q.part, &row.key))
            .map(|row| (row.key.clone(), row))
            .collect();
        for row in self.matching(q) {
            if row.key != except {
                found.insert(row.key.clone(), row);
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

/// A run's checkpoint metadata, newest first, merging the overlay's rows into the stored
/// cursor; stored rows are still read one at a time as consumed.
pub(crate) struct OverlaidMetas<'a> {
    stored: CheckpointMetas<'a>,
    peeked: Option<HistoryRow>,
    pending: Peekable<std::vec::IntoIter<HistoryRow>>,
    overlay: Arc<Overlay>,
}

impl Iterator for OverlaidMetas<'_> {
    type Item = Result<CheckpointMeta>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.peeked.is_none() {
            match self.stored.next_row() {
                None => break,
                Some(Err(error)) => return Some(Err(error)),
                Some(Ok(row)) if self.overlay.holds(row.part, &row.key) => {}
                Some(Ok(row)) => self.peeked = Some(row),
            }
        }
        let newer = match (self.pending.peek(), &self.peeked) {
            (Some(pending), Some(stored)) => pending.key > stored.key,
            (pending, _) => pending.is_some(),
        };
        let row = if newer {
            self.pending.next()
        } else {
            self.peeked.take()
        }?;
        Some(serde_json::from_value(row.doc).map_err(Into::into))
    }
}

fn container_ordinal(key: &str) -> Option<usize> {
    key.split('/').next()?.parse().ok()
}

impl TaskHeadMut {
    pub(crate) fn overlaid(&self) -> Result<TaskHead> {
        let stored: BTreeMap<Key, HistoryRow> = super::head_finish::projection_rows(&self.base)?
            .into_iter()
            .map(|row| ((row.part, row.key.clone()), row))
            .collect();
        let mut rows: BTreeMap<Key, Option<HistoryRow>> = self.rows.clone();
        for row in self.pending_rows()? {
            let key = (row.part, row.key.clone());
            if stored.get(&key) != Some(&row) {
                rows.insert(key, Some(row));
            }
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

    /// Run `run`'s checkpoint metadata, newest first, as this mutation would leave it.
    pub(crate) fn checkpoint_metas_overlaid(&self, run: &str) -> Result<OverlaidMetas<'_>> {
        let overlaid = self.overlaid()?;
        let owner = overlaid.run_owner(run)?;
        let overlay = overlaid.overlay.clone().unwrap_or_default();
        let pending = match &owner {
            Some(owner) => overlay
                .matching(&RowQuery::under(TaskPart::RunCheckpoint, owner.clone()).newest_first()),
            None => Vec::new(),
        };
        Ok(OverlaidMetas {
            stored: CheckpointMetas::of_owner(&self.head, owner),
            peeked: None,
            pending: pending.into_iter().peekable(),
            overlay,
        })
    }

    pub(crate) fn has_pending_writes(&self) -> bool {
        !self.rows.is_empty() || !self.demoted.is_empty() || self.head != *self.base
    }
}
