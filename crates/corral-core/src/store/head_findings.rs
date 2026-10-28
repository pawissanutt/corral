//! Derived `findings` rows of a head mutation (plan T-253 section 4, "Derived rows and
//! indexes"): every review child or plan review row the mutation writes or deletes rewrites its
//! finding in the same `UpdateTask`, so the rows always equal `task_findings` of the full task.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::codec::seq;
use super::head::{last_ordinal, projection};
use super::head_mut::{Demoted, TaskHeadMut};
use super::{PlanReviewHead, RunHead};
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::work::Counter;
use crate::{EmittedFinding, FindingSource, finding_key};

type Key = (TaskPart, String);
/// A finding row to write (or delete) and the stored finding it replaces.
pub(super) type FindingWrite = (Key, Option<HistoryRow>, Option<HistoryRow>);

fn container_ordinal(key: &str) -> Result<usize> {
    key.split('/')
        .next()
        .unwrap_or(key)
        .parse()
        .with_context(|| format!("row key {key:?} has no container ordinal"))
}

fn finding_of(part: TaskPart, row: &HistoryRow, owner: &str) -> Result<Option<EmittedFinding>> {
    Ok(match part {
        TaskPart::ReviewRun => {
            let (child, _): (RunHead, _) = projection(&row.doc)?;
            EmittedFinding::of_review_head(owner, &child)
        }
        _ => {
            let review: PlanReviewHead = serde_json::from_value(row.doc.clone())?;
            EmittedFinding::of_plan_review(
                owner,
                &review.id,
                &review.reviewer,
                review.verdict,
                review.notes.as_deref(),
            )
        }
    })
}

impl TaskHeadMut {
    /// The finding writes for the review rows in `writes`, with the stored finding each replaces.
    pub(super) fn finding_writes(
        &self,
        writes: &BTreeMap<Key, Option<HistoryRow>>,
        stored: &BTreeMap<Key, Option<HistoryRow>>,
    ) -> Result<Vec<FindingWrite>> {
        let mut out = Vec::new();
        for ((part, key), new) in writes {
            let (source, container) = match part {
                TaskPart::ReviewRun => (FindingSource::Review, TaskPart::Run),
                TaskPart::PlanReview => (FindingSource::Plan, TaskPart::Plan),
                _ => continue,
            };
            let old = stored.get(&(*part, key.clone())).cloned().flatten();
            let derive = |row: Option<&HistoryRow>, owner: &str| {
                row.map(|row| finding_of(*part, row, owner))
                    .transpose()
                    .map(Option::flatten)
            };
            let (before, after) = (derive(old.as_ref(), "")?, derive(new.as_ref(), "")?);
            if before == after {
                continue;
            }
            let s = container_ordinal(key)?;
            let owner = self.container_id(container, s, writes)?;
            let row_key = finding_key(source, s, last_ordinal(key)?);
            let row = |finding: Option<EmittedFinding>| {
                finding
                    .map(|f| super::head_keyed::finding_row(row_key.clone(), &f))
                    .transpose()
            };
            out.push((
                (TaskPart::Finding, row_key.clone()),
                row(derive(new.as_ref(), &owner)?)?,
                row(derive(old.as_ref(), &owner)?)?,
            ));
        }
        Ok(out)
    }

    /// The id of the coding run or plan at ordinal `s`, read by key only when this mutation
    /// does not hold it.
    fn container_id(
        &self,
        part: TaskPart,
        s: usize,
        writes: &BTreeMap<Key, Option<HistoryRow>>,
    ) -> Result<String> {
        let id_of = |row: &HistoryRow| {
            row.doc
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .context("a container row has no id")
        };
        if let Some(Some(row)) = writes.get(&(part, seq(s))) {
            return id_of(row);
        }
        let held = match part {
            TaskPart::Run => [&self.head, &self.base].into_iter().find_map(|head| {
                head.current_run
                    .as_ref()
                    .filter(|_| head.seqs.run == Some(s))
                    .map(|r| r.id.clone())
            }),
            _ => [&self.head, &self.base].into_iter().find_map(|head| {
                head.last_plan
                    .as_ref()
                    .filter(|_| head.seqs.plan == Some(s))
                    .map(|p| p.id.clone())
            }),
        };
        let demoted = self.demoted.iter().find_map(|demoted| match demoted {
            Demoted::Run(n, run) if *n == s && part == TaskPart::Run => Some(run.id.clone()),
            Demoted::Plan(n, plan) if *n == s && part == TaskPart::Plan => Some(plan.id.clone()),
            _ => None,
        });
        if let Some(id) = held.or(demoted) {
            return Ok(id);
        }
        let rows = self
            .head
            .query(&RowQuery::key(part, seq(s)), 1, Counter::HistoryRowDecodes)?;
        id_of(
            rows.first()
                .context("a review row's container is missing")?,
        )
    }
}
