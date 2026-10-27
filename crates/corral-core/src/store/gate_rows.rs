//! Gate attempt rows (plan T-253 sections 3, 7 and 8): a gate's identity, commands, context,
//! log and result documents are exact-byte blobs referenced by one `gate_attempts` row, which
//! `GateStart`, `GateLaunched` and `GateResult` commit; its control files live in a registered
//! scratch directory that is closed with the result and reclaimed once nothing uses it.
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use serde_json::Value;

use super::Store;
use crate::backend::{
    ApplyError, GateAttempt, GateResult as GateResultOp, HistoryRow, Operation, PageReq,
    ReservationUse, RowWrite, ScratchOp, ScratchState, SystemClock, TaskChange, TaskNum, TaskPart,
    UpdateForm, gate_key,
};
use crate::blob::{PutBlob, Putter};
use crate::blob_ref::BlobRef;
use crate::scratch::ScratchArea;

const CONFLICT_RETRIES: u32 = 16;

/// The result of one gate attempt, as a task update commits it (`GateResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct GateOutcome {
    pub run: String,
    pub attempt: String,
    pub result: String,
    pub failed_cmd: Option<String>,
    pub log: Option<BlobRef>,
    pub result_doc: Option<BlobRef>,
    /// Open scratch registrations the result closes.
    pub close_scratch: Vec<String>,
}

/// The current, open row of a started attempt, as a task change writes it.
pub(super) fn start_row(attempt: &GateAttempt) -> Result<RowWrite> {
    Ok(RowWrite::Put(HistoryRow {
        part: TaskPart::GateAttempt,
        key: attempt.key(),
        current: true,
        open: attempt.ended.is_none(),
        doc: serde_json::to_value(attempt).context("encoding gate attempt row")?,
        index: Vec::new(),
    }))
}

impl GateOutcome {
    pub(super) fn operation(self, change: TaskChange, form: UpdateForm) -> GateResultOp {
        GateResultOp {
            change,
            form,
            run: self.run,
            attempt: self.attempt,
            result: self.result,
            failed_cmd: self.failed_cmd,
            log: self.log,
            result_doc: self.result_doc,
            context: None,
            ended: Timestamp::now(),
            close_scratch: self.close_scratch,
        }
    }
}
const FIRST_DEFER: Duration = Duration::from_secs(60);
const MAX_DEFER: Duration = Duration::from_secs(3600);

/// What [`Store::reclaim_scratch`] did with one registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScratchReclaim {
    /// Deleted (or already absent) and recorded `reclaimed`.
    Reclaimed,
    /// Still in use; recorded `deferred` with this reason.
    Deferred(String),
    /// Open, deferred and not yet due, being reclaimed, already reclaimed, failed or unknown.
    Skipped,
}

impl Store {
    /// Puts `bytes` as a blob for the gate operation that will reference it: pass the returned
    /// reservation in that operation's change and the reference in its row.
    pub fn put_gate_blob(&self, bytes: &[u8]) -> Result<PutBlob> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot store gate evidence");
        self.check_runtime()?;
        let blobs = self.blob_store()?;
        Putter::new(self.backend(), &blobs, &SystemClock)?.put(&mut &bytes[..], &mut |attempt| {
            // Busy only while GC deletes this exact content; it finishes in one pass.
            if attempt > 600 {
                bail!("blob GC kept gate evidence busy for a minute");
            }
            std::thread::sleep(Duration::from_millis(100));
            Ok(())
        })
    }

    /// Commits one gate operation of `task` that `build` makes around a change keeping the
    /// task head as it is and carrying `events` and the blob `reservations`. A concurrent
    /// write of the task between reading its head and applying retries with the new head;
    /// a refused precondition is returned as an error. Call it outside a task update.
    pub fn apply_gate(
        &self,
        task: &str,
        events: Vec<(&str, Value)>,
        reservations: Vec<ReservationUse>,
        build: impl Fn(TaskChange) -> Operation,
    ) -> Result<()> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot record gates");
        self.check_runtime()?;
        let id = Self::normalize_id(task)?;
        let num = TaskNum(super::id_number(&id));
        let events = events
            .into_iter()
            .map(|(kind, detail)| {
                super::events::new_event(self.attributed_event(Some(&id), kind, detail)?)
            })
            .collect::<Result<Vec<_>>>()?;
        for _ in 0..CONFLICT_RETRIES {
            let head = self
                .backend()
                .view()?
                .head(num)?
                .with_context(|| format!("task {id} does not exist"))?;
            let change = TaskChange {
                num,
                expected_rev: head.header.rev,
                fields: head.header.fields,
                doc: head.doc,
                summary: head.summary,
                rows: Vec::new(),
                events: events.clone(),
                records: Vec::new(),
                aggregates: Vec::new(),
                reservations: reservations.clone(),
                conditions: Vec::new(),
            };
            match self.backend().apply(build(change)) {
                Ok(_) => return Ok(()),
                Err(ApplyError::Conflict(_)) => continue,
                Err(error) => return Err(anyhow!("recording gate of {id}: {error}")),
            }
        }
        bail!("recording gate of {id}: the task kept changing")
    }

    /// Stages a new attempt row into the open task update, which then commits as one
    /// `GateStart`: the update's marker and its start event (required), the row and its blob
    /// references (consuming `reservations`). Within an authorized update the row joins the
    /// authorized change instead. Call it from inside `update`; the attempt must not have a
    /// row yet.
    pub fn stage_gate_start(
        &self,
        attempt: &GateAttempt,
        reservations: Vec<ReservationUse>,
    ) -> Result<()> {
        let attempt = attempt.clone();
        self.stage_gate(|staged| {
            anyhow::ensure!(
                staged.start.is_none(),
                "a task update commits at most one gate start"
            );
            staged.start = Some(attempt);
            staged.reservations.extend(reservations);
            Ok(())
        })
    }

    /// Stages an attempt's result into the open task update, which then commits as one
    /// `GateResult`: the update's marker and events, the ended row with its log and result
    /// references (consuming `reservations`) and the closure of `outcome.close_scratch`.
    /// Call it from inside `update`.
    pub fn stage_gate_result(
        &self,
        outcome: GateOutcome,
        reservations: Vec<ReservationUse>,
    ) -> Result<()> {
        self.stage_gate(|staged| {
            anyhow::ensure!(
                staged.result.is_none(),
                "a task update commits at most one gate result"
            );
            staged.result = Some(outcome);
            staged.reservations.extend(reservations);
            Ok(())
        })
    }

    /// The gate attempt row of `task` for `run`/`attempt`, read as that one row.
    pub fn gate_attempt(
        &self,
        task: &str,
        run: &str,
        attempt: &str,
    ) -> Result<Option<GateAttempt>> {
        let mut page = PageReq::first(1);
        let key = Some(gate_key(run, attempt));
        self.task_rows(task, TaskPart::GateAttempt, None, key, &mut page)?
            .pop()
            .map(|doc| serde_json::from_value(doc).context("decoding gate attempt row"))
            .transpose()
    }

    /// The original bytes of a live blob; `None` when it is not live.
    pub fn blob_bytes(&self, blob: &BlobRef) -> Result<Option<Vec<u8>>> {
        let Some(mut reader) = self.blob_reader(blob.hash())? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading blob {blob}"))?;
        Ok(Some(bytes))
    }

    /// Reclaims scratch registration `id` when it is closed, or deferred and due, and
    /// `in_use` finds nothing still using its directory; otherwise records `deferred` with
    /// `in_use`'s reason and a doubling recheck delay. Open registrations are never touched.
    pub fn reclaim_scratch(
        &self,
        id: &str,
        owner: &str,
        in_use: impl FnOnce(&Path) -> Result<Option<String>>,
    ) -> Result<ScratchReclaim> {
        let layout = self.storage_layout()?;
        let area = ScratchArea::open(self.backend(), &layout)?;
        let Some(row) = area.row(id)? else {
            return Ok(ScratchReclaim::Skipped);
        };
        let now = Timestamp::now();
        let due = match row.state {
            ScratchState::Closed => true,
            ScratchState::Deferred => row.next_check_at.is_none_or(|at| at <= now),
            _ => false,
        };
        if !due {
            return Ok(ScratchReclaim::Skipped);
        }
        let dir = area.root().join(&row.path);
        if let Some(reason) = in_use(&dir)? {
            let delay = match (row.state, row.closed) {
                (ScratchState::Deferred, Some(closed)) => {
                    let waited = now.duration_since(closed).unsigned_abs();
                    waited.clamp(FIRST_DEFER, MAX_DEFER)
                }
                _ => FIRST_DEFER,
            };
            self.backend()
                .apply(Operation::Scratch(ScratchOp::Defer {
                    id: id.into(),
                    from: row.state,
                    reason: reason.clone(),
                    next_check_at: now.checked_add(jiff::SignedDuration::try_from(delay)?)?,
                }))
                .map_err(|error| anyhow!("defer scratch {id}: {error}"))?;
            return Ok(ScratchReclaim::Deferred(reason));
        }
        area.reclaim(id, owner, now)?;
        Ok(ScratchReclaim::Reclaimed)
    }
}
