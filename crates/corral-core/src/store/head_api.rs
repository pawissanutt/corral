//! The head-scoped facade (plan T-253 section 4, "Sweep of the kept facade family"): heads,
//! board deltas and the `update_head` family, under the same leases as their full twins.

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result, bail, ensure};
use jiff::Timestamp;
use serde_json::Value;

use super::codec::task_fields;
use super::head::{PinnedView, TaskHead};
use super::head_mut::TaskHeadMut;
use super::write::apply_error;
use super::{Store, TaskHeadFields, id_number};
use crate::Task;
use crate::backend::{
    ChangePage, ChangeSeq, HeadQuery, Outcome, PageReq, ReadView, StatusCount, TaskChange, TaskNum,
    UpdateForm,
};
use crate::work::{self, Counter};

/// The open board on one view, or what changed on it after `since`.
#[derive(Debug, Clone)]
pub struct Board {
    pub seq: ChangeSeq,
    pub since: Option<ChangeSeq>,
    pub heads: Vec<TaskHead>,
    /// Tasks changed after `since` that left the open board.
    pub left: Vec<String>,
    pub status_counts: Vec<StatusCount>,
}

/// The task a head describes with every history vector empty, for the checks and digests that
/// read only head fields.
pub(crate) fn shell_task(fields: &TaskHeadFields) -> Result<Task> {
    let mut task: Task = serde_json::from_value(serde_json::to_value(fields)?)?;
    task.body = fields.body.clone();
    Ok(task)
}

fn head_doc_of(fields: &TaskHeadFields) -> Result<Value> {
    super::codec::head_doc(&shell_task(fields)?)
}

impl Store {
    fn pinned_view(&self) -> Result<PinnedView> {
        if let Some(frozen) = &self.frozen {
            return Ok(Arc::clone(&frozen.view));
        }
        Ok(Arc::new(Mutex::new(self.view()?)))
    }

    /// A view opened after `num` committed revision `rev`, refusing one that observes another
    /// revision: the task lease keeps leased writers off the task, so only a writer bypassing
    /// it can make the returned head's keyed reads describe another state.
    fn view_at_rev(&self, num: TaskNum, rev: u64, id: &str) -> Result<PinnedView> {
        let view = self.pinned_view()?;
        let stored = {
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            guard
                .head(num)?
                .with_context(|| format!("{id} vanished after its write committed"))?
        };
        work::decoded(
            Counter::TaskHeadDecodes,
            super::read::doc_bytes(&stored.doc) + super::read::doc_bytes(&stored.summary),
            work::Subject::Plain,
        );
        ensure!(
            stored.header.rev == rev,
            "{id} moved from revision {rev} to {} after its write committed; a writer bypassed the task lease",
            stored.header.rev
        );
        Ok(view)
    }

    fn head_on(view: &PinnedView, num: TaskNum) -> Result<Option<TaskHead>> {
        let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
        let head = TaskHead::read(guard.as_ref(), num)?;
        drop(guard);
        Ok(head.map(|mut head| {
            head.view = Some(Arc::clone(view));
            head
        }))
    }

    /// A raw read view of the store's latest committed state.
    ///
    /// # Errors
    /// Returns view errors.
    pub fn view(&self) -> Result<Box<dyn ReadView + Send>> {
        if let Some(frozen) = &self.frozen {
            return Ok(Box::new(super::snapshot::SharedView(Arc::clone(
                &frozen.view,
            ))));
        }
        work::counted(Counter::ViewsOpened, 1);
        Ok(self
            .db
            .backend
            .view_with_max_age(self.config.storage.view_max_age())?)
    }

    /// The head of one task; its keyed reads use the view it was read on.
    ///
    /// # Errors
    /// Returns "task T-042 not found" when missing, or read errors.
    pub fn head(&self, id: &str) -> Result<TaskHead> {
        let id = Self::normalize_id(id)?;
        let view = self.pinned_view()?;
        Self::head_on(&view, TaskNum(id_number(&id)))?
            .filter(|head| head.task.id == id)
            .with_context(|| format!("task {id} not found"))
    }

    /// Heads matching `q`, all on one view.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn heads(&self, q: &HeadQuery) -> Result<Vec<TaskHead>> {
        let view = self.pinned_view()?;
        let docs = {
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            let mut docs = Vec::new();
            let mut page = PageReq::first(500);
            loop {
                let found = guard.heads(q, page.clone())?;
                docs.extend(found.items);
                match found.next {
                    Some(next) => page.after = Some(next),
                    None => break docs,
                }
            }
        };
        let mut heads = Vec::new();
        for doc in docs {
            if work::in_hot_scope() {
                work::decoded(
                    Counter::TaskHeadDecodes,
                    super::read::doc_bytes(&doc.doc) + super::read::doc_bytes(&doc.summary),
                    work::Subject::Plain,
                );
            }
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            let head = TaskHead::from_doc(guard.as_ref(), doc)?;
            drop(guard);
            if let Some(mut head) = head {
                head.view = Some(Arc::clone(&view));
                heads.push(head);
            }
        }
        Ok(heads)
    }

    /// The open board, or the heads changed after `since`, on one view.
    ///
    /// # Errors
    /// Returns read errors, including a feed gap when `since` was pruned.
    pub fn board(&self, since: Option<ChangeSeq>) -> Result<Board> {
        let view = self.pinned_view()?;
        let delta = {
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            guard.board(since)?
        };
        let mut heads = Vec::new();
        for doc in delta.heads {
            work::decoded(
                Counter::TaskHeadDecodes,
                super::read::doc_bytes(&doc.doc) + super::read::doc_bytes(&doc.summary),
                work::Subject::Plain,
            );
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            let head = TaskHead::from_doc(guard.as_ref(), doc)?;
            drop(guard);
            if let Some(mut head) = head {
                head.view = Some(Arc::clone(&view));
                heads.push(head);
            }
        }
        Ok(Board {
            seq: delta.seq,
            since: delta.since,
            heads,
            left: delta
                .left
                .iter()
                .map(|num| format!("T-{:03}", num.0))
                .collect(),
            status_counts: delta.status_counts,
        })
    }

    /// Change-feed entries after `after`.
    ///
    /// # Errors
    /// Returns read errors, including a feed gap when `after` was pruned.
    pub fn changes_since(&self, after: ChangeSeq, limit: u32) -> Result<ChangePage> {
        Ok(self.view()?.changes(after, limit)?)
    }

    /// Reads one head under its task lock.
    ///
    /// # Errors
    /// Returns lookup, lock or callback errors.
    pub fn read_locked_head<T>(
        &self,
        id: &str,
        f: impl FnOnce(&TaskHead) -> Result<T>,
    ) -> Result<T> {
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        f(&self.head(&id)?)
    }

    /// Takes the same leases as [`Store::update`], reads the head from a fresh view, applies
    /// `f` and writes only the head, the touched current containers and the rows `f` added or
    /// changed; history stays as it is.
    ///
    /// # Errors
    /// Returns lookup, callback, check or write errors.
    pub fn update_head<F>(&self, id: &str, f: F) -> Result<TaskHead>
    where
        F: FnOnce(&mut TaskHeadMut) -> Result<()>,
    {
        self.try_update_head(id, |head| {
            f(head)?;
            Ok(true)
        })?
        .context("update_head wrote nothing")
    }

    /// Like [`Store::update_head`], but `f` decides on the fresh head whether to write.
    ///
    /// # Errors
    /// Returns lookup, callback, check or write errors.
    pub fn try_update_head<F>(&self, id: &str, f: F) -> Result<Option<TaskHead>>
    where
        F: FnOnce(&mut TaskHeadMut) -> Result<bool>,
    {
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        let mut head = TaskHeadMut::new(self.head(&id)?);
        let _unit = self.event_unit();
        if !f(&mut head)? {
            return Ok(None);
        }
        self.write_head(head).map(Some)
    }

    /// The head form of [`Store::dispatch_update`].
    ///
    /// # Errors
    /// Returns lookup, callback, check or write errors.
    pub fn dispatch_update_head<R>(
        &self,
        id: &str,
        f: impl FnOnce(&Store, &mut TaskHeadMut) -> Result<R>,
    ) -> Result<(TaskHead, R)> {
        let _board = self.lock("dispatch")?;
        let id = Self::normalize_id(id)?;
        let _task = self.lock(&id)?;
        let mut fresh = self.clone();
        fresh.reload_config_if_changed()?;
        let mut head = TaskHeadMut::new(fresh.head(&id)?);
        let _unit = fresh.event_unit();
        let result = f(&fresh, &mut head)?;
        Ok((fresh.write_head(head)?, result))
    }

    pub(crate) fn write_head(&self, head: TaskHeadMut) -> Result<TaskHead> {
        let _unit = self.event_unit();
        if self.frozen.is_some() {
            bail!(
                "a read-only task snapshot cannot save {}",
                head.head.task.id
            );
        }
        self.check_runtime()?;
        let before = shell_task(&head.base.task)?;
        let base_doc = head_doc_of(&head.base.task)?;
        let base_summary = serde_json::to_value(&head.base.summary)?;
        let (num, rev) = (head.base.num, head.base.rev);
        let (mut new, rows, history_edit) = head.finish()?;
        let mut after = shell_task(&new.task)?;
        before.check_scope_unchanged(&after)?;
        let decision = self.protect_owner_change(&before, &after)?;
        let doc = head_doc_of(&new.task)?;
        let summary = serde_json::to_value(&new.summary)?;
        let mut fields = task_fields(&after)?;
        if !rows.is_empty() || doc != base_doc || summary != base_summary {
            let now = Timestamp::now();
            new.task.updated = now;
            after.updated = now;
            fields.updated = now;
        }
        let edited = crate::edited::fields(&before, &after);
        for decision in decision {
            self.log_event(
                Some(&after.id),
                "idea_decided",
                serde_json::json!({
                    "action": decision.detail["action"], "reason": decision.detail["reason"],
                    "actor": "owner", "owner_ok": true, "decision_at": decision.ts,
                    "task_updated": after.updated,
                    "into": after.idea_decision.as_ref().and_then(|d| d.into.as_deref())
                }),
            )?;
        }
        let callback_events = self.buffered_events();
        if !edited.is_empty() {
            self.log_event(
                Some(&after.id),
                "edited",
                serde_json::json!({"fields": edited}),
            )?;
        }
        self.buffer_events(callback_events)?;
        let events = self.buffered_task_events(&new.task.id, new.task.updated)?;
        let records = self.buffered_records();
        let conditions = self.task_conditions(&after.id, before.status, after.status)?;
        if events.is_empty()
            && records.is_empty()
            && conditions.is_empty()
            && !self.has_buffered_authorization()
            && !self.has_staged_gate()
            && rows.is_empty()
            && doc == base_doc
            && summary == base_summary
        {
            return Ok(new);
        }
        let committed = self
            .db
            .backend
            .apply(self.task_operation(
                TaskChange {
                    num,
                    expected_rev: rev,
                    fields,
                    doc,
                    summary,
                    rows,
                    events,
                    records,
                    aggregates: Vec::new(),
                    reservations: Vec::new(),
                    conditions,
                },
                if history_edit {
                    UpdateForm::Full
                } else {
                    UpdateForm::Head
                },
            )?)
            .map_err(apply_error)
            .with_context(|| format!("saving {}", new.task.id))?;
        if let Outcome::Task { rev, .. } = committed.outcome {
            new.rev = rev;
        }
        new.view = Some(self.view_at_rev(num, new.rev, &new.task.id)?);
        Ok(new)
    }
}
