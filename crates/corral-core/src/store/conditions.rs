//! Condition runs and slim event text (plan T-253 section 9). The five noise kinds are
//! observed as condition runs: an identical repeat counts on its open run without an event,
//! and each kind's resolution closes the run with one `<kind>_cleared` event.

use anyhow::{Context, Result};
use serde_json::Value;

use super::Store;
use crate::Status;
use crate::backend::{
    CONDITION_KINDS, ClearCondition, ConditionOutcome, ConditionQuery, ConditionRun,
    ConditionWrite, ObserveCondition, Operation, Outcome, PageReq,
};
use crate::blob_ref::BlobRef;

/// Event text of this many bytes or more is stored as a blob and referenced by a `BlobRef`.
pub const SLIM_TEXT_BYTES: usize = 16 * 1024;

/// Whether `kind` is observed as a condition run rather than appended as an event.
pub fn is_condition_kind(kind: &str) -> bool {
    CONDITION_KINDS.contains(&kind)
}

impl Store {
    /// Observes condition `kind` of `task` (`None` for workspace kinds) with `detail`: a repeat
    /// of the open run's detail counts without an event; any other detail appends one event
    /// that closes the open run as `changed` and opens a new run. Inside a task update the
    /// observation is applied only after that update commits, and discarded if it refuses;
    /// it then returns `None`.
    ///
    /// # Errors
    /// Returns attribution, snapshot or backend errors.
    pub fn observe_condition(
        &self,
        task: Option<&str>,
        kind: &str,
        detail: Value,
    ) -> Result<Option<ConditionOutcome>> {
        let event = super::events::new_event(self.attributed_event(task, kind, detail)?)?;
        self.condition(ConditionWrite::Observe(ObserveCondition {
            task: event.task.unwrap_or_default(),
            kind: event.kind,
            detail: event.detail,
            at: event.ts,
            actor: event.actor,
            via: event.via,
        }))
        .with_context(|| format!("observing {kind}"))
    }

    /// Clears condition `kind` of `task`: an open run is closed as `cleared` by one
    /// `<kind>_cleared` event carrying its count; without one nothing is written. Inside a
    /// task update it is applied after that update commits, and returns `None`.
    ///
    /// # Errors
    /// Returns attribution, snapshot or backend errors.
    pub fn clear_condition(
        &self,
        task: Option<&str>,
        kind: &str,
    ) -> Result<Option<ConditionOutcome>> {
        let event = super::events::new_event(self.attributed_event(task, kind, Value::Null)?)?;
        self.condition(ConditionWrite::Clear(ClearCondition {
            task: event.task.unwrap_or_default(),
            kind: event.kind,
            at: event.ts,
            actor: event.actor,
            via: event.via,
        }))
        .with_context(|| format!("clearing {kind}"))
    }

    fn condition(&self, write: ConditionWrite) -> Result<Option<ConditionOutcome>> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot record conditions");
        if self.has_record_unit() {
            self.buffer_condition(write)?;
            return Ok(None);
        }
        self.apply_condition(match write {
            ConditionWrite::Observe(op) => Operation::ObserveCondition(op),
            ConditionWrite::Clear(op) => Operation::ClearCondition(op),
        })
        .map(Some)
    }

    fn apply_condition(&self, op: Operation) -> Result<ConditionOutcome> {
        anyhow::ensure!(!self.is_snapshot(), "a snapshot cannot record conditions");
        self.check_runtime()?;
        let committed = self
            .db
            .backend
            .apply(op)
            .map_err(super::write::apply_error)?;
        match committed.outcome {
            Outcome::Condition(outcome) => Ok(outcome),
            other => anyhow::bail!("condition operation returned {other:?}"),
        }
    }

    /// Condition runs in `opened_seq` order on one view; `task` narrows them to one task
    /// (`Some("")` selects workspace kinds).
    ///
    /// # Errors
    /// Returns view errors.
    pub fn condition_runs(&self, task: Option<&str>) -> Result<Vec<ConditionRun>> {
        let view = self.view()?;
        let query = ConditionQuery {
            task: task.map(str::to_owned),
            ..ConditionQuery::default()
        };
        let mut runs = Vec::new();
        let mut page = PageReq::first(500);
        loop {
            let got = view.conditions(&query, page.clone())?;
            runs.extend(got.items);
            match got.next {
                Some(next) => page.after = Some(next),
                None => return Ok(runs),
            }
        }
    }

    /// The conditions a task write commits with: those its callback buffered, then, when the
    /// write leaves Ready, the clears of its dispatch conditions (`queued`, `dispatch_failed`).
    pub(super) fn task_conditions(
        &self,
        task: &str,
        from: Status,
        to: Status,
    ) -> Result<Vec<ConditionWrite>> {
        let mut writes = self.buffered_conditions();
        if from == Status::Ready && to != Status::Ready {
            for kind in ["queued", "dispatch_failed"] {
                let event = super::events::new_event(self.attributed_event(
                    Some(task),
                    kind,
                    Value::Null,
                )?)?;
                writes.push(ConditionWrite::Clear(ClearCondition {
                    task: task.to_owned(),
                    kind: event.kind,
                    at: event.ts,
                    actor: event.actor,
                    via: event.via,
                }));
            }
        }
        Ok(writes)
    }

    /// Operations that carry no task change cannot carry a buffered condition.
    pub(super) fn refuse_buffered_conditions(&self, what: &str) -> Result<()> {
        anyhow::ensure!(
            self.buffered_conditions().is_empty(),
            "a condition commits with a task update, not with {what}"
        );
        Ok(())
    }

    /// `text` as an event value inside a task update: text of [`SLIM_TEXT_BYTES`] or more is
    /// stored once as a blob whose reservation joins the update, and becomes its `BlobRef`.
    ///
    /// # Errors
    /// Returns blob store errors, or an error outside a task update.
    pub fn event_text(&self, text: &str) -> Result<Value> {
        if text.len() < SLIM_TEXT_BYTES {
            return Ok(Value::String(text.to_owned()));
        }
        let blob = self.put_gate_blob(text.as_bytes())?;
        self.stage_gate(|staged| {
            staged.reservations.push(blob.reservation);
            Ok(())
        })?;
        Ok(Value::String(blob.blob.to_string()))
    }

    /// The text of an event value written by [`Store::event_text`]: a `BlobRef` reads its
    /// blob, any other string is returned as it is; `None` for a non-string or a blob that is
    /// no longer live.
    ///
    /// # Errors
    /// Returns blob read or UTF-8 errors.
    pub fn resolve_text(&self, value: &Value) -> Result<Option<String>> {
        let Some(text) = value.as_str() else {
            return Ok(None);
        };
        let Ok(blob) = text.parse::<BlobRef>() else {
            return Ok(Some(text.to_owned()));
        };
        self.blob_bytes(&blob)?
            .map(|bytes| String::from_utf8(bytes).with_context(|| format!("decoding {blob}")))
            .transpose()
    }
}
