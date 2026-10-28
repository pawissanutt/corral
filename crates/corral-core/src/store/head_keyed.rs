//! Keyed top-level rows, inbox scalars, agent records and emitted findings of a head (plan
//! T-253 section 4, "`TaskRef` and `TaskMut`"): each read decodes at most the rows it returns.

use anyhow::Result;
use serde::de::DeserializeOwned;

use super::codec::seq;
use super::head::{TaskHead, projection};
use super::read::doc_bytes;
use super::{AttemptHead, PlanRunHead, RunHead};
use crate::agent::{AgentGeneration, AgentTurn, ContainerRef};
use crate::backend::{HistoryRow, PageReq, RowQuery, TaskPart};
use crate::inbox::Recipient;
use crate::work::{self, Counter, Subject};
use crate::{EmittedFinding, FindingSource, InboxHead};

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
            return Ok(Some(current.clone()));
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
            return Ok(Some(current.clone()));
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
            return Ok(Some(current.clone()));
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
        let Some(current) = self.seqs.run.filter(|s| *s > 1) else {
            return Ok(None);
        };
        let rows = self.query(
            &RowQuery::key(TaskPart::Run, seq(current - 1)),
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

    fn agent_record<T: DeserializeOwned>(
        &self,
        container: &ContainerRef,
        kind: &str,
        token: String,
    ) -> Result<Option<T>> {
        let (task, run, child) = match container {
            ContainerRef::Worker { task, run } => (task, run, None),
            ContainerRef::Reviewer { task, run, review } => (task, run, Some(review.as_str())),
            ContainerRef::Pm => return Ok(None),
        };
        if *task != self.task.id {
            return Ok(None);
        }
        let Some(owner) = self.owner_key(run, child)? else {
            return Ok(None);
        };
        let rows = self.query(
            &RowQuery::under(TaskPart::AgentRecord, format!("{owner}/{kind}/")).with_index(token),
            1,
            Counter::NestedRowDecodes,
        )?;
        rows.into_iter()
            .next()
            .map(|row| Ok(serde_json::from_value(row.doc)?))
            .transpose()
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

    /// Every owner comment in order, for the prompts' Owner comments section only.
    pub fn emitted_comments(&self) -> Result<Vec<crate::TaskComment>> {
        let _emitted = work::emitted();
        let view = self.pinned()?;
        let rows = view
            .rows(
                self.num,
                &RowQuery::part(TaskPart::Comment),
                PageReq::first(u32::MAX),
            )?
            .items;
        for row in &rows {
            work::decoded(
                Counter::HistoryRowDecodes,
                doc_bytes(&row.doc),
                Subject::Emitted,
            );
        }
        rows.into_iter()
            .map(|row| Ok(serde_json::from_value(row.doc)?))
            .collect()
    }

    /// Every finding of `source` but `exclude`, for the two findings prompt sections only.
    pub fn emitted_findings(
        &self,
        source: FindingSource,
        exclude: Option<&str>,
    ) -> Result<Vec<EmittedFinding>> {
        let _emitted = work::emitted();
        let prefix = match source {
            FindingSource::Review => "run:",
            FindingSource::Plan => "plan:",
        };
        let rows = {
            let view = self.pinned()?;
            view.rows(
                self.num,
                &RowQuery::under(TaskPart::Finding, prefix),
                PageReq::first(u32::MAX),
            )?
            .items
        };
        let mut findings = Vec::new();
        for row in rows {
            let finding: EmittedFinding = serde_json::from_value(row.doc.clone())?;
            if Some(finding.attempt.as_str()) == exclude {
                continue;
            }
            work::decoded(
                Counter::HistoryRowDecodes,
                doc_bytes(&row.doc),
                Subject::Emitted,
            );
            findings.push(finding);
        }
        Ok(findings)
    }
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
