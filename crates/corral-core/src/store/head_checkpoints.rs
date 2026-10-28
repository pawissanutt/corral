//! Checkpoint reads of a head (plan T-253 section 4, "Checkpoints: metadata and payload"):
//! every row a reader returns belongs to the named run, so an own-record scope of that run
//! claims it whether or not the run is the current one.

use anyhow::Result;

use super::codec::seq;
use super::head::{CheckpointMetas, TaskHead, read_rows};
use super::read::doc_bytes;
use crate::backend::{HistoryRow, PageReq, RowQuery, TaskPart};
use crate::work::{self, Counter, Subject};
use crate::{Checkpoint, CheckpointMeta, CheckpointRecord};

impl TaskHead {
    pub fn checkpoint_by_artifact(
        &self,
        run: &str,
        artifact: &str,
    ) -> Result<Option<CheckpointMeta>> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(None);
        };
        let q = RowQuery::under(TaskPart::RunCheckpoint, owner)
            .with_index(format!("artifact:{artifact}"));
        // Every row under the owner prefix is `run`'s own, current run or not.
        let view = self.pinned()?;
        let rows = read_rows(
            view.as_ref(),
            self.num,
            &self.task.id,
            &|_| Some(run.to_owned()),
            &q,
            1,
            Counter::NestedRowDecodes,
        )?;
        drop(view);
        Ok(Self::decode(self.overlaid_rows(&q, rows))?
            .into_iter()
            .next())
    }

    /// The checkpoint metadata of `run`, newest first, read one row at a time as consumed;
    /// each row is counted as `run`'s own child row.
    pub fn checkpoint_metas_newest_first(&self, run: &str) -> Result<CheckpointMetas<'_>> {
        Ok(CheckpointMetas::of_run(self, self.run_owner(run)?, run))
    }

    /// The checkpoint record `meta` names, for the resume packet only: its payload row by
    /// digest, counted as one emitted row (plan T-253 section 4, Worker-launch resume).
    pub fn emitted_checkpoint(&self, meta: &CheckpointMeta) -> Result<Option<CheckpointRecord>> {
        let _emitted = work::emitted();
        let q = RowQuery::key(TaskPart::CheckpointPayload, meta.payload_digest.clone());
        let rows = self.pinned()?.rows(self.num, &q, PageReq::first(1))?.items;
        let rows = self.overlaid_rows(&q, rows);
        for row in &rows {
            work::decoded(
                Counter::NestedRowDecodes,
                doc_bytes(&row.doc),
                Subject::Emitted,
            );
        }
        Ok(Self::decode::<Checkpoint>(rows)?
            .into_iter()
            .next()
            .map(|checkpoint| CheckpointRecord {
                checkpoint,
                artifact: meta.artifact.clone(),
            }))
    }

    /// The canonical checkpoint `ordinal` (1-based) of `run`: its meta row and one payload.
    pub fn checkpoint_payload(&self, run: &str, ordinal: u64) -> Result<Option<Checkpoint>> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(None);
        };
        let key = format!("{owner}{}", seq(usize::try_from(ordinal)?));
        // Both rows are `run`'s own: its meta row and the payload it names.
        let own = |q: &RowQuery| -> Result<Vec<HistoryRow>> {
            let view = self.pinned()?;
            let rows = read_rows(
                view.as_ref(),
                self.num,
                &self.task.id,
                &|_| Some(run.to_owned()),
                q,
                1,
                Counter::NestedRowDecodes,
            )?;
            drop(view);
            Ok(self.overlaid_rows(q, rows))
        };
        let Some(meta) = own(&RowQuery::key(TaskPart::RunCheckpoint, key))?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let meta: CheckpointMeta = serde_json::from_value(meta.doc)?;
        let payload = own(&RowQuery::key(
            TaskPart::CheckpointPayload,
            meta.payload_digest,
        ))?;
        Ok(Self::decode(payload)?.into_iter().next())
    }
}
