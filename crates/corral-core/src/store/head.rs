//! Task heads (plan T-253 section 4, "TaskHead"): a task's own fields, its current-container
//! projections and its history summary, read on one view; older rows are reached only through
//! the counted keyed reads, pending pages and cursors here, on the view the head was read from.

use std::ops::Deref;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::assemble::head_object;
use super::codec::{FACTS, SELF, child_key, cursor_token};
use super::read::{doc_bytes, run_seq_of};
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead, TaskHeadFields};
use crate::backend::{
    HistoryRow, Page, PageReq, ReadView, RowQuery, TaskHeadDoc, TaskNum, TaskPart,
};
use crate::inbox::{Recipient, SessionCursor};
use crate::work::{self, Counter, Subject};
use crate::{
    AggregateReview, CheckpointMeta, ContainerFacts, DecisionRecord, HistorySummary, ResumeReceipt,
    RunObligations, UsageEntry, WaitRecord,
};

/// The view a head was read on, shared by its keyed reads.
pub(crate) type PinnedView = Arc<Mutex<Box<dyn ReadView + Send>>>;

/// Ordinals (1-based) of the current containers, so keyed reads find their rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CurrentSeqs {
    pub(crate) run: Option<usize>,
    pub(crate) run_child: Option<usize>,
    pub(crate) plan_run: Option<usize>,
    pub(crate) plan: Option<usize>,
    pub(crate) plan_review: Option<usize>,
    pub(crate) attempt: Option<usize>,
    pub(crate) attempt_review: Option<usize>,
}

/// A head: the task's own fields, the current-container projections and the summary.
#[derive(Clone)]
pub struct TaskHead {
    pub num: TaskNum,
    pub rev: u64,
    pub task: TaskHeadFields,
    pub current_run: Option<Box<RunHead>>,
    pub last_plan_run: Option<Box<PlanRunHead>>,
    pub last_plan: Option<Box<PlanHead>>,
    pub last_aggregate_attempt: Option<Box<AttemptHead>>,
    pub summary: HistorySummary,
    pub(crate) seqs: CurrentSeqs,
    pub(crate) view: Option<PinnedView>,
    pub(crate) overlay: Option<Arc<super::head_overlay::Overlay>>,
}

impl std::fmt::Debug for TaskHead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskHead")
            .field("num", &self.num)
            .field("rev", &self.rev)
            .field("task", &self.task)
            .field("current_run", &self.current_run)
            .field("last_plan_run", &self.last_plan_run)
            .field("last_plan", &self.last_plan)
            .field("last_aggregate_attempt", &self.last_aggregate_attempt)
            .field("summary", &self.summary)
            .finish()
    }
}

impl PartialEq for TaskHead {
    fn eq(&self, other: &Self) -> bool {
        self.num == other.num
            && self.rev == other.rev
            && self.task == other.task
            && self.current_run == other.current_run
            && self.last_plan_run == other.last_plan_run
            && self.last_plan == other.last_plan
            && self.last_aggregate_attempt == other.last_aggregate_attempt
            && self.summary == other.summary
    }
}

impl Deref for TaskHead {
    type Target = TaskHeadFields;

    fn deref(&self) -> &TaskHeadFields {
        &self.task
    }
}

pub(crate) fn projection<T: DeserializeOwned>(doc: &Value) -> Result<(T, ContainerFacts)> {
    let mut map: Map<String, Value> = doc.as_object().cloned().unwrap_or_default();
    let facts = map
        .remove(FACTS)
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    Ok((serde_json::from_value(Value::Object(map))?, facts))
}

pub(crate) fn last_ordinal(key: &str) -> Result<usize> {
    key.rsplit('/')
        .next()
        .unwrap_or(key)
        .parse()
        .with_context(|| format!("row key {key:?} has no ordinal"))
}

fn first_ordinal(key: &str) -> Result<usize> {
    key.split('/')
        .next()
        .unwrap_or(key)
        .parse()
        .with_context(|| format!("row key {key:?} has no ordinal"))
}

/// Reads rows on `view`, counting each returned row as `counter` (nested history unless the
/// caller says otherwise).
pub(crate) fn read_rows(
    view: &dyn ReadView,
    num: TaskNum,
    task: &str,
    runs: &dyn Fn(&str) -> Option<String>,
    q: &RowQuery,
    limit: u32,
    counter: Counter,
) -> Result<Vec<HistoryRow>> {
    Ok(read_page(view, num, task, runs, q, PageReq::first(limit), counter)?.items)
}

/// One page of [`read_rows`], each row counted as one decode.
pub(crate) fn read_page(
    view: &dyn ReadView,
    num: TaskNum,
    task: &str,
    runs: &dyn Fn(&str) -> Option<String>,
    q: &RowQuery,
    page: PageReq,
    counter: Counter,
) -> Result<Page<HistoryRow>> {
    let page = view.rows(num, q, page)?;
    if work::in_hot_scope() {
        for row in &page.items {
            // A checkpoint payload is keyed by digest: only a caller naming its run claims it.
            let payload = (row.part == TaskPart::CheckpointPayload).then_some("");
            let run = run_seq_of(row).or(payload).and_then(runs);
            let subject = match run.as_deref() {
                Some(run) => Subject::RunChild { task, run },
                None => Subject::Plain,
            };
            work::decoded(counter, doc_bytes(&row.doc), subject);
        }
    }
    Ok(page)
}

fn one(rows: Vec<HistoryRow>) -> Option<HistoryRow> {
    rows.into_iter().next()
}

impl TaskHead {
    /// Reads the head of `num` on `view`: the head row, the current containers and their
    /// current children.
    pub(crate) fn read(view: &dyn ReadView, num: TaskNum) -> Result<Option<TaskHead>> {
        let Some(doc) = view.head(num)? else {
            return Ok(None);
        };
        if work::in_hot_scope() {
            work::decoded(
                Counter::TaskHeadDecodes,
                doc_bytes(&doc.doc) + doc_bytes(&doc.summary),
                Subject::Plain,
            );
        }
        Self::from_doc(view, doc)
    }

    pub(crate) fn from_doc(view: &dyn ReadView, doc: TaskHeadDoc) -> Result<Option<TaskHead>> {
        let num = doc.header.num;
        let id = doc.header.id.clone();
        let none = |_: &str| None;
        let current = |part: TaskPart, counter: Counter| -> Result<Option<HistoryRow>> {
            Ok(one(read_rows(
                view,
                num,
                &id,
                &none,
                &RowQuery::part(part).current(),
                1,
                counter,
            )?))
        };
        let child = |part: TaskPart, parent: &str| -> Result<Option<HistoryRow>> {
            Ok(one(read_rows(
                view,
                num,
                &id,
                &none,
                &RowQuery::under(part, format!("{parent}/")).current(),
                1,
                Counter::TaskHeadDecodes,
            )?))
        };
        let mut seqs = CurrentSeqs::default();
        let mut current_run = None;
        if let Some(row) = current(TaskPart::Run, Counter::RunDecodes)? {
            let (mut run, facts): (RunHead, _) = projection(&row.doc)?;
            run.facts = facts;
            seqs.run = Some(first_ordinal(&row.key)?);
            if let Some(child) = child(TaskPart::ReviewRun, &row.key)? {
                let (mut head, facts): (RunHead, _) = projection(&child.doc)?;
                head.facts = facts;
                seqs.run_child = Some(last_ordinal(&child.key)?);
                run.current_child = Some(Box::new(head));
            }
            current_run = Some(run);
        }
        let mut last_plan_run = None;
        if let Some(row) = current(TaskPart::PlanRun, Counter::TaskHeadDecodes)? {
            let (mut run, facts): (PlanRunHead, _) = projection(&row.doc)?;
            run.facts = facts;
            seqs.plan_run = Some(first_ordinal(&row.key)?);
            last_plan_run = Some(run);
        }
        let mut last_plan = None;
        if let Some(row) = current(TaskPart::Plan, Counter::TaskHeadDecodes)? {
            let (mut plan, facts): (PlanHead, _) = projection(&row.doc)?;
            plan.facts = facts;
            seqs.plan = Some(first_ordinal(&row.key)?);
            if let Some(review) = child(TaskPart::PlanReview, &row.key)? {
                seqs.plan_review = Some(last_ordinal(&review.key)?);
                plan.current_review = Some(serde_json::from_value(review.doc)?);
            }
            last_plan = Some(plan);
        }
        let mut last_aggregate_attempt = None;
        if let Some(row) = current(TaskPart::AggregateAttempt, Counter::TaskHeadDecodes)? {
            let (mut attempt, facts): (AttemptHead, _) = projection(&row.doc)?;
            attempt.facts = facts;
            seqs.attempt = Some(first_ordinal(&row.key)?);
            if let Some(review) = child(TaskPart::AggregateReview, &row.key)? {
                seqs.attempt_review = Some(last_ordinal(&review.key)?);
                attempt.current_review = Some(serde_json::from_value(review.doc)?);
            }
            last_aggregate_attempt = Some(attempt);
        }
        let mut task: TaskHeadFields = serde_json::from_value(Value::Object(head_object(&doc)?))
            .context("decoding the task head")?;
        task.body = super::assemble::head_body(&doc);
        Ok(Some(TaskHead {
            num,
            rev: doc.header.rev,
            task,
            current_run: current_run.map(Box::new),
            last_plan_run: last_plan_run.map(Box::new),
            last_plan: last_plan.map(Box::new),
            last_aggregate_attempt: last_aggregate_attempt.map(Box::new),
            summary: serde_json::from_value(doc.summary)?,
            seqs,
            view: None,
            overlay: None,
        }))
    }

    pub(crate) fn pinned(&self) -> Result<std::sync::MutexGuard<'_, Box<dyn ReadView + Send>>> {
        Ok(self
            .view
            .as_ref()
            .context("this head was not read from a store view")?
            .lock()
            .unwrap_or_else(PoisonError::into_inner))
    }

    /// The run id of a run seq this head knows without a read (the current run).
    pub(crate) fn known_run(&self, seq: &str) -> Option<String> {
        let current = self.seqs.run.map(super::codec::seq)?;
        (current == seq)
            .then(|| self.current_run.as_ref().map(|run| run.id.clone()))
            .flatten()
    }

    pub(crate) fn query(
        &self,
        q: &RowQuery,
        limit: u32,
        counter: Counter,
    ) -> Result<Vec<HistoryRow>> {
        let view = self.pinned()?;
        let known = |seq: &str| self.known_run(seq);
        let read = |page| {
            read_page(
                view.as_ref(),
                self.num,
                &self.task.id,
                &known,
                q,
                page,
                counter,
            )
        };
        match self.overlay.as_ref().filter(|o| o.touches(q.part)) {
            Some(overlay) => overlay.read(q, limit, read),
            None => Ok(read(PageReq::first(limit))?.items),
        }
    }

    /// [`Self::query`] of rows that all belong to run `run`, checkpoint payloads included.
    pub(crate) fn query_for_run(
        &self,
        q: &RowQuery,
        counter: Counter,
        run: &str,
    ) -> Result<Vec<HistoryRow>> {
        let view = self.pinned()?;
        let rows = read_rows(
            view.as_ref(),
            self.num,
            &self.task.id,
            &|_| Some(run.to_owned()),
            q,
            u32::MAX,
            counter,
        )?;
        Ok(self.overlaid_rows(q, rows))
    }

    /// `stored` (every stored row matching `q`) with this head's overlay applied.
    pub(crate) fn overlaid_rows(&self, q: &RowQuery, stored: Vec<HistoryRow>) -> Vec<HistoryRow> {
        match self.overlay.as_ref().filter(|o| o.touches(q.part)) {
            Some(overlay) => overlay.apply(q, stored, "", u32::MAX),
            None => stored,
        }
    }

    /// The seq of the coding run `run`: the current one without a read, else one keyed read.
    pub(crate) fn run_seq(&self, run: &str) -> Result<Option<usize>> {
        if let (Some(seq), Some(current)) = (self.seqs.run, &self.current_run)
            && current.id == run
        {
            return Ok(Some(seq));
        }
        let rows = self.query(
            &RowQuery::part(TaskPart::Run).with_index(format!("id:{run}")),
            1,
            Counter::HistoryRowDecodes,
        )?;
        rows.first().map(|row| first_ordinal(&row.key)).transpose()
    }

    fn container_seq(
        &self,
        part: TaskPart,
        current: Option<(usize, &str)>,
        id: &str,
    ) -> Result<Option<usize>> {
        if let Some((seq, current)) = current
            && current == id
        {
            return Ok(Some(seq));
        }
        let rows = self.query(
            &RowQuery::part(part).with_index(format!("id:{id}")),
            1,
            Counter::HistoryRowDecodes,
        )?;
        rows.first().map(|row| first_ordinal(&row.key)).transpose()
    }

    pub(crate) fn plan_seq(&self, plan: &str) -> Result<Option<usize>> {
        let current = self
            .seqs
            .plan
            .zip(self.last_plan.as_ref().map(|p| p.id.as_str()));
        self.container_seq(TaskPart::Plan, current, plan)
    }

    pub(crate) fn attempt_seq(&self, attempt: &str) -> Result<Option<usize>> {
        let current = self
            .seqs
            .attempt
            .zip(self.last_aggregate_attempt.as_ref().map(|a| a.id.as_str()));
        self.container_seq(TaskPart::AggregateAttempt, current, attempt)
    }

    fn child_heads(&self, rows: Vec<HistoryRow>) -> Result<Vec<RunHead>> {
        rows.iter()
            .map(|row| {
                let (mut head, facts): (RunHead, _) = projection(&row.doc)?;
                head.facts = facts;
                Ok(head)
            })
            .collect()
    }

    fn run_children(
        &self,
        run: &str,
        q: impl FnOnce(String) -> RowQuery,
        limit: u32,
    ) -> Result<Vec<RunHead>> {
        let Some(seq) = self.run_seq(run)? else {
            return Ok(Vec::new());
        };
        let rows = self.query(
            &q(format!("{}/", super::codec::seq(seq))),
            limit,
            Counter::NestedRowDecodes,
        )?;
        self.child_heads(rows)
    }

    pub fn review_child_at(&self, run: &str, n: usize) -> Result<Option<RunHead>> {
        if n == 0 { return Ok(None); }
        let Some(s) = self.run_seq(run)? else { return Ok(None); };
        let rows = self.query(&RowQuery::key(TaskPart::ReviewRun, child_key(s, n)),
            1, Counter::NestedRowDecodes)?;
        Ok(self.child_heads(rows)?.pop())
    }

    /// One review child of `run` by its id.
    pub fn review_child(&self, run: &str, id: &str) -> Result<Option<RunHead>> {
        let token = format!("id:{id}");
        Ok(self
            .run_children(
                run,
                |p| RowQuery::under(TaskPart::ReviewRun, p).with_index(token),
                1,
            )?
            .into_iter()
            .next())
    }

    /// The newest review child of `run` whose review was recorded at `at` (an availability
    /// fallback child carries its parent's record, so several may share it).
    pub fn review_child_by_review_at(
        &self,
        run: &str,
        at: jiff::Timestamp,
    ) -> Result<Option<RunHead>> {
        let token = format!("at:{at}");
        Ok(self
            .run_children(
                run,
                |p| {
                    RowQuery::under(TaskPart::ReviewRun, p)
                        .with_index(token)
                        .newest_first()
                },
                1,
            )?
            .into_iter()
            .next())
    }

    /// The superseded child of `run` that consumed the stronger-review transition on `sha`.
    pub fn stronger_source(
        &self,
        run: &str,
        sha: &str,
        exclude: Option<&str>,
    ) -> Result<Option<String>> {
        let token = format!("stronger:{sha}");
        Ok(self
            .run_children(
                run,
                |p| RowQuery::under(TaskPart::ReviewRun, p).with_index(token),
                2,
            )?
            .into_iter()
            .map(|child| child.id)
            .find(|id| Some(id.as_str()) != exclude))
    }

    /// Review children of `run` that have not ended, oldest first.
    pub fn open_review_children(&self, run: &str) -> Result<Vec<RunHead>> {
        self.run_children(
            run,
            |p| RowQuery::under(TaskPart::ReviewRun, p).open(),
            u32::MAX,
        )
    }

    /// Every review child of `run` (output-sized; outside hot scopes only).
    pub fn review_children(&self, run: &str) -> Result<Vec<RunHead>> {
        self.run_children(run, |p| RowQuery::under(TaskPart::ReviewRun, p), u32::MAX)
    }

    pub(super) fn decode<T: DeserializeOwned>(rows: Vec<HistoryRow>) -> Result<Vec<T>> {
        rows.into_iter()
            .map(|row| Ok(serde_json::from_value(row.doc)?))
            .collect()
    }

    fn plan_rows(
        &self,
        plan: &str,
        q: impl FnOnce(String) -> RowQuery,
        limit: u32,
    ) -> Result<Vec<PlanReviewHead>> {
        let Some(seq) = self.plan_seq(plan)? else {
            return Ok(Vec::new());
        };
        Self::decode(self.query(
            &q(format!("{}/", super::codec::seq(seq))),
            limit,
            Counter::NestedRowDecodes,
        )?)
    }

    pub fn plan_review(&self, plan: &str, id: &str) -> Result<Option<PlanReviewHead>> {
        let token = format!("id:{id}");
        Ok(self
            .plan_rows(
                plan,
                |p| RowQuery::under(TaskPart::PlanReview, p).with_index(token),
                1,
            )?
            .into_iter()
            .next())
    }

    /// The `n`th (1-based) review of `plan`.
    pub fn plan_review_at(&self, plan: &str, n: usize) -> Result<Option<PlanReviewHead>> {
        let Some(seq) = self.plan_seq(plan)? else {
            return Ok(None);
        };
        let key = child_key(seq, n);
        Ok(Self::decode(self.query(
            &RowQuery::key(TaskPart::PlanReview, key),
            1,
            Counter::NestedRowDecodes,
        )?)?
        .into_iter()
        .next())
    }

    /// The first review of `plan` that `reviewer` approved.
    pub fn plan_review_approved_by(
        &self,
        plan: &str,
        reviewer: &str,
    ) -> Result<Option<PlanReviewHead>> {
        let token = format!("by:{reviewer}:approve");
        Ok(self
            .plan_rows(
                plan,
                |p| RowQuery::under(TaskPart::PlanReview, p).with_index(token),
                1,
            )?
            .into_iter()
            .next())
    }

    pub fn pending_plan_reviews(&self, plan: &str) -> Result<Vec<PlanReviewHead>> {
        self.plan_rows(
            plan,
            |p| RowQuery::under(TaskPart::PlanReview, p).open(),
            u32::MAX,
        )
    }

    pub fn plan_reviews(&self, plan: &str) -> Result<Vec<PlanReviewHead>> {
        self.plan_rows(plan, |p| RowQuery::under(TaskPart::PlanReview, p), u32::MAX)
    }

    fn attempt_rows(
        &self,
        attempt: &str,
        q: impl FnOnce(String) -> RowQuery,
        limit: u32,
    ) -> Result<Vec<AggregateReview>> {
        let Some(seq) = self.attempt_seq(attempt)? else {
            return Ok(Vec::new());
        };
        Self::decode(self.query(
            &q(format!("{}/", super::codec::seq(seq))),
            limit,
            Counter::NestedRowDecodes,
        )?)
    }

    pub fn aggregate_review(&self, attempt: &str, id: &str) -> Result<Option<AggregateReview>> {
        let token = format!("id:{id}");
        Ok(self
            .attempt_rows(
                attempt,
                |p| RowQuery::under(TaskPart::AggregateReview, p).with_index(token),
                1,
            )?
            .into_iter()
            .next())
    }

    pub fn pending_aggregate_reviews(&self, attempt: &str) -> Result<Vec<AggregateReview>> {
        self.attempt_rows(
            attempt,
            |p| RowQuery::under(TaskPart::AggregateReview, p).open(),
            u32::MAX,
        )
    }

    pub fn aggregate_reviews(&self, attempt: &str) -> Result<Vec<AggregateReview>> {
        self.attempt_rows(
            attempt,
            |p| RowQuery::under(TaskPart::AggregateReview, p),
            u32::MAX,
        )
    }

    /// The owner prefix of a run's own nested rows (`<run>/000000/`).
    pub(crate) fn run_owner(&self, run: &str) -> Result<Option<String>> {
        // A run id's ordinal is fixed within one pinned view: one lookup per view serves every
        // keyed read of that run (a resume packet asks once per candidate). Entries hold the
        // view weakly, so another store's or a dropped view's entry never matches.
        type Entry = (
            std::sync::Weak<Mutex<Box<dyn ReadView + Send>>>,
            u64,
            String,
            Option<usize>,
        );
        thread_local! {
            static OWNERS: std::cell::RefCell<Vec<Entry>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let view = self.view.as_ref().filter(|_| self.overlay.is_none());
        let hit = view.and_then(|view| {
            OWNERS.with(|owners| {
                owners.borrow().iter().find_map(|(weak, num, id, seq)| {
                    (*num == self.num.0
                        && id == run
                        && weak.upgrade().is_some_and(|live| Arc::ptr_eq(&live, view)))
                    .then_some(*seq)
                })
            })
        });
        let seq = match hit {
            Some(seq) => seq,
            None => {
                let seq = self.run_seq(run)?;
                if let Some(view) = view {
                    OWNERS.with(|owners| {
                        let mut owners = owners.borrow_mut();
                        owners.retain(|(weak, ..)| weak.strong_count() > 0);
                        owners.push((Arc::downgrade(view), self.num.0, run.to_owned(), seq));
                    });
                }
                seq
            }
        };
        Ok(seq.map(|seq| format!("{}/", child_key(seq, SELF))))
    }

    fn obligation_rows(
        &self,
        run: &str,
        kind: &str,
        token: Option<String>,
        open: bool,
    ) -> Result<Vec<HistoryRow>> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(Vec::new());
        };
        self.owner_obligation_rows(&owner, kind, token, open)
    }

    fn owner_obligation_rows(
        &self,
        owner: &str,
        kind: &str,
        token: Option<String>,
        open: bool,
    ) -> Result<Vec<HistoryRow>> {
        let mut q = RowQuery::under(TaskPart::RunObligation, format!("{owner}{kind}"));
        q.index = token;
        q.open_only = open;
        self.query(
            &q,
            if q.index.is_some() { 1 } else { u32::MAX },
            Counter::NestedRowDecodes,
        )
    }

    pub fn wait(&self, run: &str, id: &str) -> Result<Option<WaitRecord>> {
        Ok(
            Self::decode(self.obligation_rows(run, "wait/", Some(format!("id:{id}")), false)?)?
                .into_iter()
                .next(),
        )
    }

    pub fn decision(&self, run: &str, id: &str) -> Result<Option<DecisionRecord>> {
        Ok(Self::decode(self.obligation_rows(
            run,
            "decision/",
            Some(format!("id:{id}")),
            false,
        )?)?
        .into_iter()
        .next())
    }

    /// The resume receipt `id` of a wait or decision of `run`.
    pub fn receipt(&self, run: &str, id: &str) -> Result<Option<ResumeReceipt>> {
        let rows = self.obligation_rows(run, "", Some(format!("receipt:{id}")), false)?;
        Ok(rows.into_iter().find_map(|row| {
            serde_json::from_value::<ResumeReceipt>(row.doc.get("receipt")?.clone()).ok()
        }))
    }

    /// Waits and decisions of `run` whose observation can still change, in order.
    pub fn open_obligations(&self, run: &str) -> Result<RunObligations> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(RunObligations::default());
        };
        Ok(RunObligations {
            slot_wait: crate::TaskRef::run_head(self, run)?.and_then(|r| r.obligations.slot_wait),
            waits: Self::decode(self.owner_obligation_rows(&owner, "wait/", None, true)?)?,
            decisions: Self::decode(self.owner_obligation_rows(
                &owner,
                "decision/",
                None,
                true,
            )?)?,
        })
    }

    /// The task's operating-contract revision, as its summary keeps it: no history is read.
    pub fn operating_contract_revision(&self) -> &str {
        &self.summary.operating_contract_revision
    }

    /// The usage entries of one owner, a run or one of its review children (output-sized).
    pub fn usage_entries(&self, run: &str, child: Option<&str>) -> Result<Vec<UsageEntry>> {
        let Some(owner) = self.owner_key(run, child)? else {
            return Ok(Vec::new());
        };
        Self::decode(self.query(
            &RowQuery::under(TaskPart::RunUsage, format!("{owner}/")),
            u32::MAX,
            Counter::NestedRowDecodes,
        )?)
    }

    /// The `<run>/<child>` key of a run (no child) or of one of its review children.
    pub(crate) fn owner_key(&self, run: &str, child: Option<&str>) -> Result<Option<String>> {
        let Some(seq) = self.run_seq(run)? else {
            return Ok(None);
        };
        let Some(child) = child else {
            return Ok(Some(child_key(seq, SELF)));
        };
        if self.seqs.run == Some(seq)
            && let (Some(n), Some(current)) = (
                self.seqs.run_child,
                self.current_run
                    .as_ref()
                    .and_then(|r| r.current_child.as_deref()),
            )
            && current.id == child
        {
            return Ok(Some(child_key(seq, n)));
        }
        let rows = self.query(
            &RowQuery::under(TaskPart::ReviewRun, format!("{}/", super::codec::seq(seq)))
                .with_index(format!("id:{child}")),
            1,
            Counter::NestedRowDecodes,
        )?;
        Ok(rows.first().map(|row| row.key.clone()))
    }

    /// The inbox row owner of a recipient on this task.
    pub(crate) fn inbox_owner(&self, recipient: &Recipient) -> Result<Option<String>> {
        Ok(match recipient {
            Recipient::Run { run, .. } => self.owner_key(run, None)?.map(|k| format!("run:{k}")),
            Recipient::ReviewRun { run, review, .. } => self
                .owner_key(run, Some(review))?
                .map(|k| format!("run:{k}")),
            Recipient::PlanRun { plan_run, .. } => {
                let current = self
                    .seqs
                    .plan_run
                    .zip(self.last_plan_run.as_ref().map(|r| r.id.as_str()));
                self.container_seq(TaskPart::PlanRun, current, plan_run)?
                    .map(|s| format!("plan_run:{}", super::codec::seq(s)))
            }
            Recipient::PlanReview { plan, review, .. } => {
                let Some(seq) = self.plan_seq(plan)? else {
                    return Ok(None);
                };
                let rows = self.query(
                    &RowQuery::under(TaskPart::PlanReview, format!("{}/", super::codec::seq(seq)))
                        .with_index(format!("id:{review}")),
                    1,
                    Counter::NestedRowDecodes,
                )?;
                rows.first().map(|row| format!("plan_review:{}", row.key))
            }
            _ => None,
        })
    }

    /// Whether the inbox of `recipient` applied effect `seq`.
    pub fn effect_applied(&self, recipient: &Recipient, seq: u64) -> Result<bool> {
        let Some(owner) = self.inbox_owner(recipient)? else {
            return Ok(false);
        };
        Ok(!self
            .query(
                &RowQuery::key(TaskPart::InboxEffect, format!("{owner}/{seq:020}")),
                1,
                Counter::NestedRowDecodes,
            )?
            .is_empty())
    }

    /// The scoped observe cursor of `recipient` for one adapter and binding.
    pub fn observe_cursor(
        &self,
        recipient: &Recipient,
        adapter: &str,
        binding: &crate::inbox::SessionBinding,
    ) -> Result<Option<SessionCursor>> {
        let Some(owner) = self.inbox_owner(recipient)? else {
            return Ok(None);
        };
        let token = cursor_token(adapter, &serde_json::to_value(binding)?)?;
        let q = RowQuery::under(TaskPart::ObserveCursor, format!("{owner}/")).with_index(token);
        Ok(Self::decode(self.query(&q, 1, Counter::NestedRowDecodes)?)?
            .into_iter()
            .next())
    }
}

/// A cursor over one run's checkpoint metadata, newest first.
pub struct CheckpointMetas<'a> {
    head: &'a TaskHead,
    owner: Option<String>,
    after: Option<String>,
    /// The run the rows belong to, when the caller named it.
    run: Option<String>,
}

impl<'a> CheckpointMetas<'a> {
    /// The metas under `owner`, each counted as `run`'s own child row.
    pub(super) fn of_run(head: &'a TaskHead, owner: Option<String>, run: &str) -> Self {
        let mut metas = Self::of_owner(head, owner);
        metas.run = Some(run.to_owned());
        metas
    }

    pub(super) fn of_owner(head: &'a TaskHead, owner: Option<String>) -> Self {
        Self {
            head,
            owner,
            after: None,
            run: None,
        }
    }

    /// The next stored row, read and counted as one decode.
    pub(super) fn next_row(&mut self) -> Option<Result<HistoryRow>> {
        let owner = self.owner.clone()?;
        let q = RowQuery::under(TaskPart::RunCheckpoint, owner).newest_first();
        let view = match self.head.pinned() {
            Ok(view) => view,
            Err(error) => return Some(Err(error)),
        };
        let page = PageReq {
            after: self.after.clone(),
            limit: 1,
        };
        let page = match view.rows(self.head.num, &q, page) {
            Ok(page) => page,
            Err(error) => return Some(Err(error.into())),
        };
        self.after = page.next.clone();
        if page.next.is_none() {
            self.owner = None;
        }
        let row = page.items.into_iter().next()?;
        let run = self
            .run
            .clone()
            .or_else(|| run_seq_of(&row).and_then(|seq| self.head.known_run(seq)));
        let subject = match run.as_deref() {
            Some(run) => Subject::RunChild {
                task: &self.head.task.id,
                run,
            },
            None => Subject::Plain,
        };
        work::decoded(Counter::NestedRowDecodes, doc_bytes(&row.doc), subject);
        Some(Ok(row))
    }
}

impl Iterator for CheckpointMetas<'_> {
    type Item = Result<CheckpointMeta>;

    fn next(&mut self) -> Option<Self::Item> {
        let row = self.next_row()?;
        Some(row.and_then(|row| Ok(serde_json::from_value(row.doc)?)))
    }
}
