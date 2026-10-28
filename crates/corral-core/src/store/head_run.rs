//! The current coding run as a whole `Run` for own-record paths (plan T-253 section 4, "Usage
//! refresh"): assembled from that run's own rows, and edited by row diff of the same rows.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::RunHead;
use super::assemble::{TaskRows, run_value};
use super::codec::{Rows, SELF, seq, split_run};
use super::head::{TaskHead, last_ordinal, projection};
use super::head_mut::{TaskHeadMut, split_panes_of_run};
use crate::Run;
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::work::Counter;

type RowMap = BTreeMap<(TaskPart, String), HistoryRow>;

const SUBTREE: [TaskPart; 6] = [
    TaskPart::ReviewRun,
    TaskPart::RunUsage,
    TaskPart::RunCheckpoint,
    TaskPart::RunSwitch,
    TaskPart::RunObligation,
    TaskPart::AgentRecord,
];

/// Every stored row of run `s`: its own row, its review children, every nested row of both,
/// their inbox rows and the checkpoint payloads they reference.
fn subtree(
    s: usize,
    fetch: &dyn Fn(&RowQuery, Counter) -> Result<Vec<HistoryRow>>,
) -> Result<RowMap> {
    fn add(rows: &mut RowMap, found: Vec<HistoryRow>) {
        for row in found {
            rows.insert((row.part, row.key.clone()), row);
        }
    }
    let mut rows = RowMap::new();
    add(
        &mut rows,
        fetch(&RowQuery::key(TaskPart::Run, seq(s)), Counter::RunDecodes)?,
    );
    for part in SUBTREE {
        let q = RowQuery::under(part, format!("{}/", seq(s)));
        add(&mut rows, fetch(&q, Counter::NestedRowDecodes)?);
    }
    for part in [TaskPart::InboxEffect, TaskPart::ObserveCursor] {
        let q = RowQuery::under(part, format!("run:{}/", seq(s)));
        add(&mut rows, fetch(&q, Counter::NestedRowDecodes)?);
    }
    let digests: Vec<String> = rows
        .values()
        .filter(|row| row.part == TaskPart::RunCheckpoint)
        .filter_map(|row| Some(row.doc.get("payload_digest")?.as_str()?.to_owned()))
        .collect();
    for digest in digests {
        let q = RowQuery::key(TaskPart::CheckpointPayload, digest);
        add(&mut rows, fetch(&q, Counter::NestedRowDecodes)?);
    }
    Ok(rows)
}

fn assemble(s: usize, rows: &RowMap) -> Result<Run> {
    let own = rows
        .get(&(TaskPart::Run, seq(s)))
        .context("the current run has no row")?;
    let view = TaskRows::new(rows.values());
    Ok(serde_json::from_value(run_value(
        &view, s, SELF, &own.doc,
    )?)?)
}

impl TaskHead {
    /// The current coding run with its review children and all of their nested records, read
    /// from that run's rows only; `None` without a current run.
    ///
    /// # Errors
    /// Returns read or decode errors.
    pub fn current_run_record(&self) -> Result<Option<Run>> {
        let (Some(s), Some(run)) = (self.seqs.run, self.current_run.as_ref()) else {
            return Ok(None);
        };
        let rows = subtree(s, &|q, counter| self.query_for_run(q, counter, &run.id))?;
        assemble(s, &rows).map(Some)
    }
}

impl TaskHeadMut {
    /// Edits the current run `run` as a whole `Run`, children included, reading only that run's
    /// rows. `edit` returns whether to write; the run's id and start may not change.
    ///
    /// # Errors
    /// Returns read, decode or callback errors, or refuses a run that is not current.
    pub fn edit_current_run(
        &mut self,
        run: &str,
        edit: impl FnOnce(&mut Run) -> Result<bool>,
    ) -> Result<bool> {
        self.adjust_projections()?;
        let s = self.current_run_seq(run)?;
        let old_rows = subtree(s, &|q, counter| self.rows_now_for_run(q, counter, run))?;
        let before = assemble(s, &old_rows)?;
        let mut after = before.clone();
        if !edit(&mut after)? {
            return Ok(false);
        }
        TaskHeadMut::ensure_unchanged_identity(&before, &after)?;
        let mut rows = Rows::new();
        split_run(
            &mut rows,
            s,
            SELF,
            &after,
            serde_json::to_value(&after)?,
            true,
        )?;
        super::write::carry_revisions(&old_rows, &mut rows.0);
        let new_rows: RowMap = rows.0;
        let own = (TaskPart::Run, seq(s));
        let current_child = new_rows
            .iter()
            .find(|((part, _), row)| *part == TaskPart::ReviewRun && row.current)
            .map(|(key, _)| key.clone());
        // The run row and its current child are written from the projection by `finish`.
        let skip = |key: &(TaskPart, String)| *key == own || Some(key) == current_child.as_ref();
        let old: RowMap = old_rows
            .iter()
            .filter(|(key, _)| !skip(key))
            .map(|(key, row)| (key.clone(), row.clone()))
            .collect();
        let new: RowMap = new_rows
            .iter()
            .filter(|(key, _)| !skip(key))
            .map(|(key, row)| (key.clone(), row.clone()))
            .collect();
        self.write_diff(&old, &new)?;
        self.write_panes(
            &split_panes_of_run(s, &before),
            &split_panes_of_run(s, &after),
        );
        let (mut head, facts): (RunHead, _) =
            projection(&new_rows.get(&own).context("the edited run has no row")?.doc)?;
        head.facts = facts;
        let mut child_seq = None;
        if let Some(key) = &current_child {
            let (mut child, facts): (RunHead, _) = projection(&new_rows[key].doc)?;
            child.facts = facts;
            head.current_child = Some(Box::new(child));
            child_seq = Some(last_ordinal(&key.1)?);
        }
        if before.total_usage() != after.total_usage() {
            self.usage_dirty = true;
        }
        self.head.current_run = Some(Box::new(head));
        self.head.seqs.run_child = child_seq;
        *self.seen = self.head.clone();
        Ok(true)
    }
}
