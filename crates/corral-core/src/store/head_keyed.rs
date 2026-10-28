//! Keyed top-level rows, inbox scalars, agent records and emitted findings of a head (plan
//! T-253 section 4, "`TaskRef` and `TaskMut`"): each read decodes at most the rows it returns.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

use super::codec::seq;
use super::head::{TaskHead, projection};
use super::read::doc_bytes;
use super::{AttemptHead, PlanRunHead, RunHead};
use crate::agent::{
    AgentGeneration, AgentQuestion, AgentReport, AgentTurn, ContainerRef, RequestReceipt,
    WakeSubscription,
};
use crate::backend::{HistoryRow, PageReq, RowQuery, TaskPart};
use crate::inbox::Recipient;
use crate::work::{self, Counter, Subject};
use crate::{EmittedFinding, FindingSource, InboxHead, RunObligations};

fn keyed<T: DeserializeOwned>(rows: Vec<HistoryRow>) -> Result<Option<(T, crate::ContainerFacts)>> {
    rows.into_iter()
        .next()
        .map(|row| projection(&row.doc))
        .transpose()
}

impl TaskHead {
    fn history_row(&self, part: TaskPart, token: String) -> Result<Vec<HistoryRow>> {
        self.query(
            &RowQuery::part(part).with_index(token),
            1,
            Counter::HistoryRowDecodes,
        )
    }

    /// The coding run `id`: the current one (with its current child) without a read, an
    /// earlier one (without a child) by one keyed row.
    pub fn run_head(&self, id: &str) -> Result<Option<RunHead>> {
        if let Some(current) = self.current_run.as_ref().filter(|r| r.id == id) {
            return Ok(Some((**current).clone()));
        }
        Ok(
            keyed::<RunHead>(self.history_row(TaskPart::Run, format!("id:{id}"))?)?.map(
                |(mut run, facts)| {
                    run.facts = facts;
                    run
                },
            ),
        )
    }

    pub fn plan_run_head(&self, id: &str) -> Result<Option<PlanRunHead>> {
        if let Some(current) = self.last_plan_run.as_ref().filter(|r| r.id == id) {
            return Ok(Some((**current).clone()));
        }
        Ok(
            keyed::<PlanRunHead>(self.history_row(TaskPart::PlanRun, format!("id:{id}"))?)?.map(
                |(mut run, facts)| {
                    run.facts = facts;
                    run
                },
            ),
        )
    }

    pub fn aggregate_attempt_head(&self, id: &str) -> Result<Option<AttemptHead>> {
        if let Some(current) = self.last_aggregate_attempt.as_ref().filter(|a| a.id == id) {
            return Ok(Some((**current).clone()));
        }
        Ok(
            keyed::<AttemptHead>(
                self.history_row(TaskPart::AggregateAttempt, format!("id:{id}"))?,
            )?
            .map(|(mut attempt, facts)| {
                attempt.facts = facts;
                attempt
            }),
        )
    }

    /// The coding run before the current one, by its key.
    pub fn previous_run_head(&self) -> Result<Option<RunHead>> {
        self.run_head_back(1)
    }

    /// The coding run `back` positions before the current one (1 is the previous run), by
    /// its key: one history row.
    pub fn run_head_back(&self, back: usize) -> Result<Option<RunHead>> {
        let Some(current) = self.seqs.run.filter(|s| *s > back) else {
            return Ok(None);
        };
        let rows = self.query(
            &RowQuery::key(TaskPart::Run, seq(current - back)),
            1,
            Counter::HistoryRowDecodes,
        )?;
        Ok(keyed::<RunHead>(rows)?.map(|(mut run, facts)| {
            run.facts = facts;
            run
        }))
    }

    /// The inbox scalars of `recipient`'s container on this task.
    pub fn inbox_head(&self, recipient: &Recipient) -> Result<Option<InboxHead>> {
        Ok(match recipient {
            Recipient::Run { run, .. } => self.run_head(run)?.map(|r| r.inbox),
            Recipient::ReviewRun { run, review, .. } => {
                let current = self
                    .current_run
                    .as_ref()
                    .filter(|r| r.id == *run)
                    .and_then(|r| r.current_child.as_deref())
                    .filter(|c| c.id == *review);
                match current {
                    Some(child) => Some(child.inbox.clone()),
                    None => self.review_child(run, review)?.map(|c| c.inbox),
                }
            }
            Recipient::PlanRun { plan_run, .. } => self.plan_run_head(plan_run)?.map(|r| r.inbox),
            Recipient::PlanReview { plan, review, .. } => {
                let current = self
                    .last_plan
                    .as_ref()
                    .filter(|p| p.id == *plan)
                    .and_then(|p| p.current_review.as_ref())
                    .filter(|r| r.id == *review);
                match current {
                    Some(review) => Some(review.inbox.clone()),
                    None => self.plan_review(plan, review)?.map(|r| r.inbox),
                }
            }
            Recipient::Pm => None,
        })
    }

    fn agent_rows(
        &self,
        container: &ContainerRef,
        kind: &str,
        token: Option<String>,
        open: bool,
    ) -> Result<Vec<HistoryRow>> {
        let (task, run, child) = match container {
            ContainerRef::Worker { task, run } => (task, run, None),
            ContainerRef::Reviewer { task, run, review } => (task, run, Some(review.as_str())),
            ContainerRef::Pm => return Ok(Vec::new()),
        };
        if *task != self.task.id {
            return Ok(Vec::new());
        }
        let Some(owner) = self.owner_key(run, child)? else {
            return Ok(Vec::new());
        };
        let mut q = RowQuery::under(TaskPart::AgentRecord, format!("{owner}/{kind}/"));
        let limit = if token.is_some() { 1 } else { u32::MAX };
        q.index = token;
        q.open_only = open;
        self.query(&q, limit, Counter::NestedRowDecodes)
    }

    fn agent_record<T: DeserializeOwned>(
        &self,
        container: &ContainerRef,
        kind: &str,
        token: String,
    ) -> Result<Option<T>> {
        Ok(decode_all(
            self.agent_rows(container, kind, Some(token), false)?
                .into_iter()
                .map(|row| row.doc)
                .collect(),
        )?
        .into_iter()
        .next())
    }

    fn agent_page<T: DeserializeOwned>(
        &self,
        container: &ContainerRef,
        kind: &str,
        open: bool,
    ) -> Result<Vec<T>> {
        decode_all(
            self.agent_rows(container, kind, None, open)?
                .into_iter()
                .map(|row| row.doc)
                .collect(),
        )
    }

    /// The native receipt of request `request_id` of `container`, by key.
    pub fn agent_receipt(
        &self,
        container: &ContainerRef,
        request_id: &str,
    ) -> Result<Option<RequestReceipt>> {
        self.agent_record(container, "receipts", format!("request:{request_id}"))
    }

    /// The native question `key` of `container`, by key.
    pub fn agent_question(
        &self,
        container: &ContainerRef,
        key: &str,
    ) -> Result<Option<AgentQuestion>> {
        self.agent_record(container, "questions", format!("key:{key}"))
    }

    /// The pending questions of `container`: its open page.
    pub fn pending_agent_questions(&self, container: &ContainerRef) -> Result<Vec<AgentQuestion>> {
        self.agent_page(container, "questions", true)
    }

    /// The wake of request `request_id` of `container`, by key.
    pub fn agent_wake(
        &self,
        container: &ContainerRef,
        request_id: &str,
    ) -> Result<Option<WakeSubscription>> {
        self.agent_record(container, "wakes", format!("request:{request_id}"))
    }

    /// The armed or pending-start wakes of `container`: its open page.
    pub fn active_agent_wakes(&self, container: &ContainerRef) -> Result<Vec<WakeSubscription>> {
        self.agent_page(container, "wakes", true)
    }

    /// Every report of `container`, its own records.
    pub fn agent_reports(&self, container: &ContainerRef) -> Result<Vec<AgentReport>> {
        self.agent_page(container, "reports", false)
    }

    /// The receipt of PM operation `request_id` on this task, by key.
    pub fn pm_receipt(&self, request_id: &str) -> Result<Option<RequestReceipt>> {
        decode_all(
            self.history_row(TaskPart::PmReceipt, format!("request:{request_id}"))?
                .into_iter()
                .map(|row| row.doc)
                .collect(),
        )
        .map(|found: Vec<RequestReceipt>| found.into_iter().next())
    }

    /// The superseded revisions of `member`'s gate, oldest first.
    pub fn member_gate_history(&self, member: &str) -> Result<Vec<crate::MemberGateRecord>> {
        decode_all(
            self.query(
                &RowQuery::under(TaskPart::MemberGateHistory, format!("{member}/")),
                u32::MAX,
                Counter::HistoryRowDecodes,
            )?
            .into_iter()
            .map(|row| row.doc)
            .collect(),
        )
    }

    /// Generation `n` of the agent bound to `container`, by key.
    pub fn agent_generation(
        &self,
        container: &ContainerRef,
        n: u32,
    ) -> Result<Option<AgentGeneration>> {
        self.agent_record(container, "generations", format!("gen:{n}"))
    }

    /// Turn `seq` of generation `generation` of the agent bound to `container`, by key.
    pub fn agent_turn(
        &self,
        container: &ContainerRef,
        generation: u32,
        seq: u64,
    ) -> Result<Option<AgentTurn>> {
        self.agent_record(container, "turns", format!("turn:{generation}:{seq}"))
    }

    /// Every row of `q` but the one at `except`, as this mutation would leave them; the
    /// stored row at `except` is not read. Each returned row counts once as an emitted
    /// decode; a stored row the overlay replaces or removes counts as a plain one.
    fn emitted_rows(
        &self,
        q: &RowQuery,
        except: &str,
        counter: Counter,
    ) -> Result<Vec<HistoryRow>> {
        let stored = {
            let view = self.pinned()?;
            view.rows_except(self.num, q, except, PageReq::first(u32::MAX))?
                .items
        };
        let rows = match self.overlay.as_ref().filter(|o| o.touches(q.part)) {
            Some(overlay) => {
                for row in stored.iter().filter(|r| overlay.holds(q.part, &r.key)) {
                    work::decoded(counter, doc_bytes(&row.doc), Subject::Plain);
                }
                overlay.apply(q, stored, except, u32::MAX)
            }
            None => stored,
        };
        for row in &rows {
            work::decoded(counter, doc_bytes(&row.doc), Subject::Emitted);
        }
        Ok(rows)
    }

    /// Every owner comment in order, for the prompts' Owner comments section only.
    pub fn emitted_comments(&self) -> Result<Vec<crate::TaskComment>> {
        let _emitted = work::emitted();
        self.emitted_rows(
            &RowQuery::part(TaskPart::Comment),
            "",
            Counter::HistoryRowDecodes,
        )?
        .into_iter()
        .map(|row| Ok(serde_json::from_value(row.doc)?))
        .collect()
    }

    /// Every wait and decision of `run` in order, decoded as `run`'s own child rows: inside
    /// `work::own_records` for that run they count as own records (plan T-253 section 4,
    /// `require_checkpoint`).
    pub fn run_obligations(&self, run: &str) -> Result<RunObligations> {
        let Some(owner) = self.run_owner(run)? else {
            return Ok(RunObligations::default());
        };
        let rows = |kind: &str| -> Result<Vec<serde_json::Value>> {
            let q = RowQuery::under(TaskPart::RunObligation, format!("{owner}{kind}"));
            Ok(self
                .query_for_run(&q, Counter::NestedRowDecodes, run)?
                .into_iter()
                .map(|row| row.doc)
                .collect())
        };
        Ok(RunObligations {
            waits: decode_all(rows("wait/")?)?,
            decisions: decode_all(rows("decision/")?)?,
        })
    }

    /// Every wait and decision of `run` in order, for the resume packet only.
    pub fn emitted_obligations(&self, run: &str) -> Result<RunObligations> {
        let _emitted = work::emitted();
        let Some(owner) = self.run_owner(run)? else {
            return Ok(RunObligations::default());
        };
        let rows = |kind: &str| -> Result<Vec<serde_json::Value>> {
            let q = RowQuery::under(TaskPart::RunObligation, format!("{owner}{kind}"));
            Ok(self
                .emitted_rows(&q, "", Counter::NestedRowDecodes)?
                .into_iter()
                .map(|row| row.doc)
                .collect())
        };
        Ok(RunObligations {
            waits: decode_all(rows("wait/")?)?,
            decisions: decode_all(rows("decision/")?)?,
        })
    }

    /// The `findings` key of the review child or plan review `id`: from the current
    /// containers without a read, else by one keyed row of its family.
    fn finding_key_of(&self, source: FindingSource, id: &str) -> Result<Option<String>> {
        let (part, current) = match source {
            FindingSource::Review => (
                TaskPart::ReviewRun,
                self.current_run
                    .as_ref()
                    .and_then(|r| r.current_child.as_deref())
                    .filter(|c| c.id == id)
                    .and(self.seqs.run.zip(self.seqs.run_child)),
            ),
            FindingSource::Plan => (
                TaskPart::PlanReview,
                self.last_plan
                    .as_ref()
                    .and_then(|p| p.current_review.as_ref())
                    .filter(|r| r.id == id)
                    .and(self.seqs.plan.zip(self.seqs.plan_review)),
            ),
        };
        if let Some((container, child)) = current {
            return Ok(Some(crate::finding_key(source, container, child)));
        }
        let rows = self.query(
            &RowQuery::part(part).with_index(format!("id:{id}")),
            1,
            Counter::NestedRowDecodes,
        )?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let container = row
            .key
            .split('/')
            .next()
            .and_then(|s| s.parse().ok())
            .context("a review row key has no container ordinal")?;
        Ok(Some(crate::finding_key(
            source,
            container,
            super::head::last_ordinal(&row.key)?,
        )))
    }

    /// Every finding of `source` but `exclude`, for the two findings prompt sections only;
    /// the excluded finding's row is not read.
    pub fn emitted_findings(
        &self,
        source: FindingSource,
        exclude: Option<&str>,
    ) -> Result<Vec<EmittedFinding>> {
        let except = match exclude {
            Some(id) => self.finding_key_of(source, id)?.unwrap_or_default(),
            None => String::new(),
        };
        let _emitted = work::emitted();
        let prefix = match source {
            FindingSource::Review => "run:",
            FindingSource::Plan => "plan:",
        };
        let rows = self.emitted_rows(
            &RowQuery::under(TaskPart::Finding, prefix),
            &except,
            Counter::HistoryRowDecodes,
        )?;
        decode_all(rows.into_iter().map(|row| row.doc).collect())
    }
}

fn decode_all<T: DeserializeOwned>(docs: Vec<serde_json::Value>) -> Result<Vec<T>> {
    Ok(docs
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn finding_row(key: String, finding: &EmittedFinding) -> Result<HistoryRow> {
    Ok(HistoryRow {
        part: TaskPart::Finding,
        key,
        current: false,
        open: false,
        doc: serde_json::to_value(finding)?,
        index: Vec::new(),
    })
}
