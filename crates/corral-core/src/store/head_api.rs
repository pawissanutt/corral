//! The head-scoped facade (plan T-253 section 4, "Sweep of the kept facade family"): heads,
//! board deltas and the `update_head` family, under the same leases as their full twins.

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result, bail, ensure};
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

impl Board {
    /// This board without its read view, to keep between frames without holding a read open;
    /// [`Store::board_since`] rebinds it.
    #[must_use]
    pub fn detach(mut self) -> Board {
        for head in &mut self.heads {
            head.view = None;
        }
        self
    }
}

/// The task a head describes with every history vector empty, for the checks and digests that
/// read only head fields.
pub(crate) fn shell_task(fields: &TaskHeadFields) -> Result<Task> {
    let mut task: Task = serde_json::from_value(serde_json::to_value(fields)?)?;
    task.body = fields.body.clone();
    Ok(task)
}

pub(crate) fn artifact_indexes(artifact: &crate::Artifact) -> Vec<String> {
    let file = crate::snapshot_name(artifact);
    let original = match file.split_once('-') {
        Some((sha, rest)) if sha.len() == 8 && sha.chars().all(|c| c.is_ascii_hexdigit()) => rest,
        _ => file,
    };
    vec![
        format!("run:{}", artifact.run),
        format!("run-path:{}:{}", artifact.run, artifact.path),
        format!("run-title:{}:{}", artifact.run, artifact.title),
        format!("run-kind:{}:{}", artifact.run, artifact.kind.as_str()),
        format!("path:{}", artifact.path),
        format!("file:{}:{file}", artifact.run),
        format!("original:{}:{original}", artifact.run),
        format!("title:{}", artifact.title),
        format!("source:{}", artifact.source),
    ]
}

impl TaskHead {
    /// Number of this task's Keep registrations marked reviewed, on this pinned view.
    pub fn reviewed_keep_artifacts(&self) -> Result<u64> {
        let view = self
            .view
            .as_ref()
            .context("reviewed count needs a pinned view")?;
        let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
        let count = guard
            .aggregates(&[crate::backend::AggregateName::ArtifactReviewed(self.num)])?
            .artifact_reviewed
            .get(&self.num)
            .copied()
            .unwrap_or(0);
        work::decoded(Counter::NestedRowDecodes, 8, work::Subject::Plain);
        Ok(count)
    }

    /// Card/list artifact scalars, using only the head's maintained facts.
    pub fn artifact_card_text(&self, field: crate::CardField) -> Option<String> {
        self.current_run.as_ref()?;
        match field {
            crate::CardField::Artifacts => {
                Some(format!("\u{25c6} {}", self.summary.current_artifacts.count))
            }
            crate::CardField::Lines => self.summary.current_artifacts.lines(),
            _ => None,
        }
    }

    /// First registration of a path in a run, preserving the full-task evidence oracle.
    pub fn artifact_in_run(&self, run: &str, path: &str) -> Result<Option<crate::Artifact>> {
        self.artifact_index(&format!("run-path:{run}:{path}"), false)
    }

    pub fn latest_artifact_kind(
        &self,
        run: &str,
        kind: crate::ArtifactKind,
    ) -> Result<Option<crate::Artifact>> {
        self.artifact_index(&format!("run-kind:{run}:{}", kind.as_str()), true)
    }

    /// A newest-first, title-keyed page. Unrelated artifacts are never decoded.
    pub fn artifacts_by_title(
        &self,
        run: &str,
        title: &str,
        page: PageReq,
    ) -> Result<crate::backend::Page<crate::Artifact>> {
        let view = self
            .view
            .as_ref()
            .context("artifact page needs a pinned view")?;
        let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
        let rows = guard.rows(
            self.num,
            &crate::backend::RowQuery::part(crate::backend::TaskPart::Artifact)
                .with_index(format!("run-title:{run}:{title}"))
                .newest_first(),
            page,
        )?;
        let mut items = Vec::new();
        for row in rows.items {
            work::decoded(
                Counter::HistoryRowDecodes,
                super::read::doc_bytes(&row.doc),
                work::Subject::Plain,
            );
            items.push(serde_json::from_value(row.doc)?);
        }
        Ok(crate::backend::Page {
            items,
            next: rows.next,
        })
    }

    /// Exactly the obligations that can produce a board card line, including uncertain
    /// deliveries with receipts. This does not change the `open_obligations` page's contract.
    pub fn card_obligations(&self, run: &str) -> Result<crate::RunObligations> {
        let Some(seq) = self.run_seq(run)? else {
            return Ok(crate::RunObligations::default());
        };
        let rows = |kind| {
            self.query(
                &crate::backend::RowQuery::under(
                    crate::backend::TaskPart::RunObligation,
                    format!("{seq:06}/000000/{kind}/"),
                )
                .with_index("card"),
                u32::MAX,
                Counter::NestedRowDecodes,
            )
        };
        let decode = |rows: Vec<crate::backend::HistoryRow>| {
            rows.into_iter().map(|row| row.doc).collect::<Vec<_>>()
        };
        Ok(crate::RunObligations {
            slot_wait: crate::TaskRef::run_head(self, run)?.and_then(|r| r.obligations.slot_wait),
            waits: decode(rows("wait")?)
                .into_iter()
                .map(serde_json::from_value)
                .collect::<std::result::Result<_, _>>()?,
            decisions: decode(rows("decision")?)
                .into_iter()
                .map(serde_json::from_value)
                .collect::<std::result::Result<_, _>>()?,
        })
    }

    /// One plan revision, projected without its review history.
    pub fn plan_by_id(&self, id: &str) -> Result<Option<super::PlanHead>> {
        if let Some(plan) = self.last_plan.as_ref().filter(|p| p.id == id) {
            return Ok(Some((**plan).clone()));
        }
        self.query(
            &crate::backend::RowQuery::part(crate::backend::TaskPart::Plan)
                .with_index(format!("id:{id}")),
            1,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .next()
        .map(|row| {
            let (mut plan, facts): (super::PlanHead, _) = super::head::projection(&row.doc)?;
            plan.facts = facts;
            Ok(plan)
        })
        .transpose()
    }

    /// Artifacts of one run, on the head's pinned view.
    pub fn artifacts_of(&self, run: &str) -> Result<Vec<crate::Artifact>> {
        self.query(
            &crate::backend::RowQuery::part(crate::backend::TaskPart::Artifact)
                .with_index(format!("run:{run}")),
            u32::MAX,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .map(|row| Ok(serde_json::from_value(row.doc)?))
        .collect()
    }

    /// Artifacts of one run with their 0-based registration ordinals over the whole task, in
    /// registration order, on the head's pinned view.
    pub fn artifact_rows_of(&self, run: &str) -> Result<Vec<(usize, crate::Artifact)>> {
        self.query(
            &crate::backend::RowQuery::part(crate::backend::TaskPart::Artifact)
                .with_index(format!("run:{run}")),
            u32::MAX,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .map(|row| {
            let ordinal: usize = row.key.parse().context("artifact row key")?;
            let index = ordinal.checked_sub(1).context("artifact row ordinal 0")?;
            Ok((index, serde_json::from_value(row.doc)?))
        })
        .collect()
    }

    /// One artifact by its stored path, without decoding other artifacts.
    pub fn artifact_by_path(&self, path: &str) -> Result<Option<crate::Artifact>> {
        self.artifact_index(&format!("path:{path}"), false)
    }

    /// All records of a stored path, including repeated registrations of the same snapshot.
    pub fn artifacts_by_path(&self, path: &str) -> Result<Vec<crate::Artifact>> {
        self.query(
            &crate::backend::RowQuery::part(crate::backend::TaskPart::Artifact)
                .with_index(format!("path:{path}")),
            u32::MAX,
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .map(|row| Ok(serde_json::from_value(row.doc)?))
        .collect()
    }

    /// First exact snapshot filename in a run, or the last original filename match.
    pub fn artifact_by_file(&self, run: &str, file: &str) -> Result<Option<crate::Artifact>> {
        if let Some(artifact) = self.artifact_index(&format!("file:{run}:{file}"), false)? {
            return Ok(Some(artifact));
        }
        self.artifact_index(&format!("original:{run}:{file}"), true)
    }

    /// Most recently appended artifact with the anchor's title or source.
    pub fn latest_artifact(&self, anchor: &crate::Artifact) -> Result<crate::Artifact> {
        // Row order, not wall time, defines follow-latest.
        let rows = |token: String| {
            self.query(
                &crate::backend::RowQuery::part(crate::backend::TaskPart::Artifact)
                    .with_index(token)
                    .newest_first(),
                1,
                Counter::HistoryRowDecodes,
            )
        };
        let title = rows(format!("title:{}", anchor.title))?.into_iter().next();
        let source = rows(format!("source:{}", anchor.source))?
            .into_iter()
            .next();
        let latest = [title, source]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.key.cmp(&b.key));
        latest
            .map(|row| Ok(serde_json::from_value(row.doc)?))
            .unwrap_or_else(|| Ok(anchor.clone()))
    }

    fn artifact_index(&self, token: &str, newest: bool) -> Result<Option<crate::Artifact>> {
        let mut query =
            crate::backend::RowQuery::part(crate::backend::TaskPart::Artifact).with_index(token);
        query.newest_first = newest;
        self.query(&query, 1, Counter::HistoryRowDecodes)?
            .into_iter()
            .next()
            .map(|row| Ok(serde_json::from_value(row.doc)?))
            .transpose()
    }

    /// A compatibility value containing the head fields and each current container as its
    /// vector's only element. Nested history is empty except the current child/review.
    ///
    /// This does not hydrate history. Counts, usage, costs and open-child state must be read
    /// from this head's facts/summary, not inferred from the shell's vectors.
    pub fn to_shell_task(&self) -> Result<Task> {
        fn run(head: &super::RunHead) -> Result<crate::Run> {
            let mut value: crate::Run = serde_json::from_value(serde_json::to_value(head)?)?;
            if let Some(child) = head.current_child.as_deref() {
                value.review_runs.push(run(child)?);
            }
            Ok(value)
        }
        let mut task = shell_task(&self.task)?;
        if let Some(head) = &self.current_run {
            task.runs.push(run(head)?);
        }
        if let Some(head) = &self.last_plan_run {
            task.plan_runs
                .push(serde_json::from_value(serde_json::to_value(head)?)?);
        }
        if let Some(head) = &self.last_plan {
            let mut plan: crate::PlanRecord = serde_json::from_value(serde_json::to_value(head)?)?;
            if let Some(review) = &head.current_review {
                plan.reviews
                    .push(serde_json::from_value(serde_json::to_value(review)?)?);
            }
            task.plans.push(plan);
        }
        if let Some(head) = &self.last_aggregate_attempt {
            let mut attempt: crate::AggregateAttempt =
                serde_json::from_value(serde_json::to_value(head)?)?;
            if let Some(review) = &head.current_review {
                attempt.reviews.push(review.clone());
            }
            task.aggregate_attempts.push(attempt);
        }
        Ok(task)
    }
}

fn head_doc_of(fields: &TaskHeadFields) -> Result<Value> {
    super::codec::head_doc(&shell_task(fields)?)
}

/// A head of a listing, or `None` after reporting one whose stored documents do not decode, as
/// the full listing does, so one damaged task cannot blank a board or a PM pass. Read-view
/// failures still propagate.
fn decode_or_skip(
    view: &dyn ReadView,
    doc: crate::backend::TaskHeadDoc,
) -> Result<Option<TaskHead>> {
    let id = doc.header.id.clone();
    match TaskHead::from_doc(view, doc) {
        Ok(head) => Ok(head),
        Err(error) if error.downcast_ref::<crate::backend::ViewError>().is_some() => Err(error),
        Err(error) => {
            crate::messages::report(Some(&id), "store", format!("skipping {id}: {error:#}"));
            Ok(None)
        }
    }
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

    /// The head of task `id` when it exists; its keyed reads use the view it was read on.
    ///
    /// # Errors
    /// Returns id or read errors.
    pub fn head_if_exists(&self, id: &str) -> Result<Option<TaskHead>> {
        let id = Self::normalize_id(id)?;
        let view = self.pinned_view()?;
        Ok(Self::head_on(&view, TaskNum(id_number(&id)))?.filter(|head| head.task.id == id))
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
        Self::decode_heads(&view, docs)
    }

    /// The first `limit` heads matching `q` in the query's order, on one view; nothing past
    /// them is read or decoded.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn heads_page(&self, q: &HeadQuery, limit: u32) -> Result<Vec<TaskHead>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let view = self.pinned_view()?;
        let docs = {
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            guard.heads(q, PageReq::first(limit))?.items
        };
        Self::decode_heads(&view, docs)
    }

    fn decode_heads(
        view: &PinnedView,
        docs: Vec<crate::backend::TaskHeadDoc>,
    ) -> Result<Vec<TaskHead>> {
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
            let head = decode_or_skip(guard.as_ref(), doc)?;
            drop(guard);
            if let Some(mut head) = head {
                head.view = Some(Arc::clone(view));
                heads.push(head);
            }
        }
        Ok(heads)
    }

    /// The open board as of now: `prev`'s heads, with every task changed after `prev.seq` read
    /// again and those that left the open board dropped. Only changed heads are decoded; a
    /// pruned feed reads the whole open board instead.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn board_since(&self, prev: &Board) -> Result<Board> {
        let view = self.pinned_view()?;
        let delta = {
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            if guard.seq() < prev.seq {
                // Another store now lives at this root: nothing of `prev` carries over.
                drop(guard);
                return self.board(None);
            }
            match guard.board(Some(prev.seq)) {
                Ok(delta) => delta,
                Err(crate::backend::FeedError::Gap { .. }) => {
                    drop(guard);
                    return self.board(None);
                }
                Err(error) => return Err(error.into()),
            }
        };
        // Every changed head replaces its previous entry, including one that no longer decodes
        // and is skipped: a full board would not show it either.
        let changed_ids: Vec<String> = delta
            .heads
            .iter()
            .map(|doc| doc.header.id.clone())
            .collect();
        let changed = Self::board_on(&view, delta)?;
        let gone: std::collections::BTreeSet<&str> = changed_ids
            .iter()
            .map(String::as_str)
            .chain(changed.left.iter().map(String::as_str))
            .collect();
        let mut heads: Vec<TaskHead> = prev
            .heads
            .iter()
            .filter(|head| !gone.contains(head.id.as_str()))
            .cloned()
            .map(|mut head| {
                head.view = Some(Arc::clone(&view));
                head
            })
            .collect();
        heads.extend(changed.heads.iter().cloned());
        heads.sort_by_key(|head| head.num);
        Ok(Board { heads, ..changed })
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
        Self::board_on(&view, delta)
    }

    fn board_on(view: &PinnedView, delta: crate::backend::BoardDelta) -> Result<Board> {
        let mut heads = Vec::new();
        for doc in delta.heads {
            work::decoded(
                Counter::TaskHeadDecodes,
                super::read::doc_bytes(&doc.doc) + super::read::doc_bytes(&doc.summary),
                work::Subject::Plain,
            );
            let guard = view.lock().unwrap_or_else(PoisonError::into_inner);
            let head = decode_or_skip(guard.as_ref(), doc)?;
            drop(guard);
            if let Some(mut head) = head {
                head.view = Some(Arc::clone(view));
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

    /// Tasks that recorded pane `pane`, or a pane in workspace `workspace` (`token` is
    /// `pane:<id>` or `workspace:<id>`), by id; no row is decoded.
    ///
    /// # Errors
    /// Returns read errors.
    pub fn pane_owners(&self, token: &str) -> Result<Vec<String>> {
        let view = self.view()?;
        let mut owners = std::collections::BTreeSet::new();
        // Pane references, and the cleanup ledger's panes their records no longer name.
        for part in [
            crate::backend::TaskPart::PaneRef,
            crate::backend::TaskPart::CleanupPane,
        ] {
            owners.extend(view.token_owners(part, token, 64)?);
        }
        Ok(owners
            .into_iter()
            .map(|num| format!("T-{:03}", num.0))
            .collect())
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

    pub(crate) fn write_head(&self, mut head: TaskHeadMut) -> Result<TaskHead> {
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
        let preserve_updated = head.preserve_updated;
        let mut normalized = shell_task(&head.head.task)?;
        super::normalize::task(&mut normalized, Some(&before), self.now())?;
        head.head.task = crate::task_head_fields(&normalized);
        let (mut new, rows, history_edit) = head.finish()?;
        let mut after = shell_task(&new.task)?;
        before.check_scope_unchanged(&after)?;
        let decision = self.protect_owner_change(&before, &after)?;
        let doc = head_doc_of(&new.task)?;
        let summary = serde_json::to_value(&new.summary)?;
        let mut fields = task_fields(&after)?;
        if !preserve_updated && (!rows.is_empty() || doc != base_doc || summary != base_summary) {
            let now = self.now();
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
