//! Resumable source enumeration with one short metadata view per page.

use anyhow::{Context, Result, ensure};
use corral_core::backend::{PageReq, RowQuery, TaskNum, TaskPart};
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::content_index::{Record, follow};

const FAMILIES: [TaskPart; 5] = [
    TaskPart::Comment,
    TaskPart::Run,
    TaskPart::ReviewRun,
    TaskPart::PlanReview,
    TaskPart::Artifact,
];

/// Only owned identity fields and the durable family/page cursor survive a batch.
#[derive(Serialize, Deserialize)]
pub(super) struct TaskScan {
    id: String,
    num: u64,
    title: String,
    updated: jiff::Timestamp,
    family: usize,
    after: Option<String>,
}

pub(super) fn begin(ctx: &Ctx, id: &str) -> Result<Option<(TaskScan, Record)>> {
    let Some(head) = ctx.store.head_if_exists(id)? else {
        return Ok(None);
    };
    let state = TaskScan {
        id: head.task.id.clone(),
        num: head.num.0,
        title: head.task.title.clone(),
        updated: head.task.updated,
        family: 0,
        after: None,
    };
    let body = head.task.body.clone();
    drop(head);
    let record = Record::text_of(
        (&state.id, &state.title, state.updated),
        None,
        "specs",
        "spec",
        body,
    );
    Ok(Some((state, record)))
}

pub(super) fn next(ctx: &Ctx, state: &mut TaskScan) -> Result<(Vec<Record>, bool)> {
    let Some(&part) = FAMILIES.get(state.family) else {
        return Ok((Vec::new(), true));
    };
    let (rows, cursor) = {
        let view = ctx.store.view().context("opening catalog metadata view")?;
        let page = view
            .rows(
                TaskNum(state.num),
                &RowQuery::part(part),
                PageReq {
                    after: state.after.clone(),
                    limit: 16,
                },
            )
            .context("reading catalog metadata page")?;
        let mut rows = Vec::with_capacity(page.items.len());
        for row in page.items {
            let parent = if matches!(part, TaskPart::ReviewRun | TaskPart::PlanReview) {
                let (key, _) = row
                    .key
                    .split_once('/')
                    .context("child row without parent key")?;
                let parent_part = if part == TaskPart::ReviewRun {
                    TaskPart::Run
                } else {
                    TaskPart::Plan
                };
                let parent = view
                    .rows(
                        TaskNum(state.num),
                        &RowQuery::key(parent_part, key),
                        PageReq::first(1),
                    )
                    .context("reading catalog parent row")?
                    .items
                    .into_iter()
                    .next()
                    .context("catalog child row without parent")?;
                Some(
                    parent.doc["id"]
                        .as_str()
                        .context("catalog parent row without id")?
                        .to_owned(),
                )
            } else {
                None
            };
            rows.push((row, parent));
        }
        (rows, page.next)
    };
    // Neither availability checks nor source derivation retain the metadata view.
    let ident = (state.id.as_str(), state.title.as_str(), state.updated);
    let mut records = Vec::new();
    for (row, parent) in rows {
        match part {
            TaskPart::Comment => {
                let ordinal = ordinal(&row.key)?;
                let comment: corral_core::TaskComment =
                    serde_json::from_value(row.doc).context("decoding catalog comment")?;
                records.push(Record::text_of(
                    ident,
                    None,
                    "reviews",
                    &format!("comment:{ordinal}"),
                    comment.text,
                ));
            }
            TaskPart::Run => {
                let run: corral_core::RunHead =
                    serde_json::from_value(row.doc).context("decoding catalog run")?;
                follow::run_records(ident, &run, &run.id, &mut records);
            }
            TaskPart::ReviewRun => {
                let (_, child) = row
                    .key
                    .split_once('/')
                    .context("review run without child ordinal")?;
                let parent = parent.context("review run without parent id")?;
                let key = format!("{parent}:reviewer:{}", ordinal(child)?);
                let run: corral_core::RunHead =
                    serde_json::from_value(row.doc).context("decoding catalog review run")?;
                follow::run_records(ident, &run, &key, &mut records);
            }
            TaskPart::PlanReview => {
                let review: corral_core::PlanReviewHead =
                    serde_json::from_value(row.doc).context("decoding catalog plan review")?;
                records.push(follow::plan_review_record(
                    ident,
                    &parent.context("plan review without parent id")?,
                    &review,
                ));
            }
            TaskPart::Artifact => {
                let artifact: corral_core::Artifact =
                    serde_json::from_value(row.doc).context("decoding catalog artifact")?;
                if artifact.purged_at.is_none()
                    && crate::artifact_storage::content_available(ctx, &artifact)?
                {
                    records.push(Record::artifact(&state.id, &artifact));
                }
            }
            _ => unreachable!("catalog source family is fixed"),
        }
    }
    state.after = cursor;
    if state.after.is_none() {
        state.family += 1;
    }
    Ok((records, state.family == FAMILIES.len()))
}

fn ordinal(key: &str) -> Result<usize> {
    let ordinal: usize = key.parse().context("invalid catalog row ordinal")?;
    ensure!(ordinal > 0, "catalog row ordinal must be one-based");
    Ok(ordinal - 1)
}
