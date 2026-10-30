//! [`TaskRef`] over the full task's vectors, with the selection rules of the stored rows a
//! head reads: projections are the documents the codec stores for each container.

use std::borrow::Cow;

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::de::DeserializeOwned;

use super::codec::{Rows, SELF, attempt_rows, child_key, plan_rows, plan_run_rows, seq, split_run};
use super::head::projection;
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead};
use crate::agent::{
    AgentGeneration, AgentQuestion, AgentReport, AgentTurn, ContainerRef, RequestReceipt,
    WakeSubscription,
};
use crate::backend::TaskPart;
use crate::inbox::{InboxState, Recipient, SessionBinding, SessionCursor};
use crate::{
    AggregateReview, Artifact, Checkpoint, CheckpointMeta, DecisionRecord, EmittedFinding,
    FindingSource, HistorySummary, InboxHead, ResumeReceipt, Run, RunObligations, Task,
    TaskComment, TaskRef, WaitRecord,
};

pub(crate) trait WithFacts {
    fn with_facts(&mut self, facts: crate::ContainerFacts);
}

macro_rules! with_facts {
    ($($ty:ty),*) => {
        $(impl WithFacts for $ty {
            fn with_facts(&mut self, facts: crate::ContainerFacts) {
                self.facts = facts;
            }
        })*
    };
}

with_facts!(RunHead, PlanRunHead, PlanHead, AttemptHead);

fn decode<T: DeserializeOwned + WithFacts>(rows: &Rows, part: TaskPart, key: &str) -> Result<T> {
    let row = rows
        .0
        .get(&(part, key.to_owned()))
        .with_context(|| format!("a split container has no {part:?} row {key}"))?;
    let (mut value, facts): (T, crate::ContainerFacts) = projection(&row.doc)?;
    value.with_facts(facts);
    Ok(value)
}

pub(crate) fn run_projection(task: &Task, i: usize, with_child: bool) -> Result<RunHead> {
    let run = task.runs.get(i).context("no such run")?;
    let n = i + 1;
    let mut rows = Rows::new();
    split_run(
        &task.id,
        &mut rows,
        n,
        SELF,
        run,
        serde_json::to_value(run)?,
        true,
    )?;
    let mut head: RunHead = decode(&rows, TaskPart::Run, &seq(n))?;
    if with_child && !run.review_runs.is_empty() {
        let child = decode(
            &rows,
            TaskPart::ReviewRun,
            &child_key(n, run.review_runs.len()),
        )?;
        head.current_child = Some(Box::new(child));
    }
    Ok(head)
}

pub(crate) fn child_projection(task: &Task, i: usize, c: usize) -> Result<RunHead> {
    child_projections(task, i)?
        .into_iter()
        .nth(c)
        .context("no such review child")
}

pub(crate) fn plan_review_projection(task: &Task, i: usize, r: usize) -> Result<PlanReviewHead> {
    review_projection(&plan_split(task, i)?, i, r + 1)
}

fn child_projections(task: &Task, i: usize) -> Result<Vec<RunHead>> {
    let run = &task.runs[i];
    let mut rows = Rows::new();
    split_run(
        &task.id,
        &mut rows,
        i + 1,
        SELF,
        run,
        serde_json::to_value(run)?,
        true,
    )?;
    (1..=run.review_runs.len())
        .map(|c| decode(&rows, TaskPart::ReviewRun, &child_key(i + 1, c)))
        .collect()
}

pub(crate) fn plan_run_projection(task: &Task, i: usize) -> Result<PlanRunHead> {
    let run = &task.plan_runs[i];
    let mut rows = Rows::new();
    plan_run_rows(&mut rows, i + 1, run, serde_json::to_value(run)?, true)?;
    decode(&rows, TaskPart::PlanRun, &seq(i + 1))
}

fn plan_split(task: &Task, i: usize) -> Result<Rows> {
    let plan = &task.plans[i];
    let mut rows = Rows::new();
    plan_rows(&mut rows, i + 1, plan, serde_json::to_value(plan)?, true)?;
    Ok(rows)
}

pub(crate) fn plan_projection(task: &Task, i: usize, with_review: bool) -> Result<PlanHead> {
    let rows = plan_split(task, i)?;
    let mut head: PlanHead = decode(&rows, TaskPart::Plan, &seq(i + 1))?;
    let reviews = task.plans[i].reviews.len();
    if with_review && reviews > 0 {
        head.current_review = Some(review_projection(&rows, i, reviews)?);
    }
    Ok(head)
}

fn review_projection(rows: &Rows, i: usize, r: usize) -> Result<PlanReviewHead> {
    let row = rows
        .0
        .get(&(TaskPart::PlanReview, child_key(i + 1, r)))
        .context("a split plan has no review row")?;
    Ok(serde_json::from_value(row.doc.clone())?)
}

fn plan_reviews(task: &Task, plan: &str) -> Result<Vec<PlanReviewHead>> {
    let Some(i) = task.plans.iter().position(|p| p.id == plan) else {
        return Ok(Vec::new());
    };
    let rows = plan_split(task, i)?;
    (1..=task.plans[i].reviews.len())
        .map(|r| review_projection(&rows, i, r))
        .collect()
}

pub(crate) fn attempt_projection(task: &Task, i: usize, with_review: bool) -> Result<AttemptHead> {
    let attempt = &task.aggregate_attempts[i];
    let mut rows = Rows::new();
    attempt_rows(
        &mut rows,
        i + 1,
        attempt,
        serde_json::to_value(attempt)?,
        true,
    )?;
    let mut head: AttemptHead = decode(&rows, TaskPart::AggregateAttempt, &seq(i + 1))?;
    if with_review {
        head.current_review = attempt.reviews.last().cloned();
    }
    Ok(head)
}

fn run_index(task: &Task, run: &str) -> Option<usize> {
    task.runs.iter().position(|r| r.id == run)
}

fn find_run<'a>(task: &'a Task, run: &str) -> Option<&'a Run> {
    task.runs.iter().find(|r| r.id == run)
}

fn children_where(
    task: &Task,
    run: &str,
    keep: impl Fn(&Run) -> bool,
    limit: usize,
) -> Result<Vec<RunHead>> {
    let Some(i) = run_index(task, run) else {
        return Ok(Vec::new());
    };
    let heads = child_projections(task, i)?;
    Ok(task.runs[i]
        .review_runs
        .iter()
        .zip(heads)
        .filter(|(child, _)| keep(child))
        .take(limit)
        .map(|(_, head)| head)
        .collect())
}

pub(crate) fn inbox_state<'a>(task: &'a Task, recipient: &Recipient) -> Option<&'a InboxState> {
    match recipient {
        Recipient::Run { run, .. } => Some(&find_run(task, run)?.inbox),
        Recipient::ReviewRun { run, review, .. } => Some(
            &find_run(task, run)?
                .review_runs
                .iter()
                .find(|c| c.id == *review)?
                .inbox,
        ),
        Recipient::PlanRun { plan_run, .. } => {
            Some(&task.plan_runs.iter().find(|r| r.id == *plan_run)?.inbox)
        }
        Recipient::PlanReview { plan, review, .. } => Some(
            &task
                .plans
                .iter()
                .find(|p| p.id == *plan)?
                .reviews
                .iter()
                .find(|r| r.id == *review)?
                .inbox,
        ),
        Recipient::Pm => None,
    }
}

pub(crate) fn inbox_head_of(state: &InboxState) -> InboxHead {
    InboxHead {
        owner_header: state.owner_header.clone(),
        next_seq: state.next_seq,
        next_envelope: state.next_envelope,
        open_envelope: state.open_envelope,
        replacement_envelope: state.replacement_envelope,
        binding: state.binding.clone(),
        observe_cursor: state.observe_cursor.clone(),
        settled: state.settled,
    }
}

fn checkpoint_metas(task: &Task, run: &str) -> Result<Vec<CheckpointMeta>> {
    let Some(run) = find_run(task, run) else {
        return Ok(Vec::new());
    };
    run.checkpoints
        .iter()
        .enumerate()
        .map(|(i, record)| Ok(CheckpointMeta::of(u64::try_from(i + 1)?, record)?))
        .collect()
}

impl TaskRef for Task {
    fn current_run_head(&self) -> Result<Option<Cow<'_, RunHead>>> {
        match self.runs.len() {
            0 => Ok(None),
            n => Ok(Some(Cow::Owned(run_projection(self, n - 1, true)?))),
        }
    }

    fn last_plan_run_head(&self) -> Result<Option<Cow<'_, PlanRunHead>>> {
        match self.plan_runs.len() {
            0 => Ok(None),
            n => Ok(Some(Cow::Owned(plan_run_projection(self, n - 1)?))),
        }
    }

    fn last_plan_head(&self) -> Result<Option<Cow<'_, PlanHead>>> {
        match self.plans.len() {
            0 => Ok(None),
            n => Ok(Some(Cow::Owned(plan_projection(self, n - 1, true)?))),
        }
    }

    fn last_aggregate_attempt_head(&self) -> Result<Option<Cow<'_, AttemptHead>>> {
        match self.aggregate_attempts.len() {
            0 => Ok(None),
            n => Ok(Some(Cow::Owned(attempt_projection(self, n - 1, true)?))),
        }
    }

    fn history_summary(&self) -> Result<Cow<'_, HistorySummary>> {
        Ok(Cow::Owned(HistorySummary::of(self)?))
    }

    fn runs_len(&self) -> Result<usize> {
        Ok(self.runs.len())
    }

    fn plan_runs_len(&self) -> Result<usize> {
        Ok(self.plan_runs.len())
    }

    fn plans_len(&self) -> Result<usize> {
        Ok(self.plans.len())
    }

    fn aggregate_attempts_len(&self) -> Result<usize> {
        Ok(self.aggregate_attempts.len())
    }

    fn comments_len(&self) -> Result<usize> {
        Ok(self.comments.len())
    }

    fn head_fields(&self) -> Result<Cow<'_, crate::TaskHeadFields>> {
        Ok(Cow::Owned(crate::task_head_fields(self)))
    }

    fn contract_revision(&self) -> Result<String> {
        crate::operating::operating_contract_revision(self)
    }

    fn run_head(&self, id: &str) -> Result<Option<RunHead>> {
        run_index(self, id)
            .map(|i| run_projection(self, i, i + 1 == self.runs.len()))
            .transpose()
    }

    fn plan_run_head(&self, id: &str) -> Result<Option<PlanRunHead>> {
        self.plan_runs
            .iter()
            .position(|r| r.id == id)
            .map(|i| plan_run_projection(self, i))
            .transpose()
    }

    fn plan_head(&self, id: &str) -> Result<Option<PlanHead>> {
        self.plans
            .iter()
            .position(|p| p.id == id)
            .map(|i| plan_projection(self, i, i + 1 == self.plans.len()))
            .transpose()
    }

    fn aggregate_attempt_head(&self, id: &str) -> Result<Option<AttemptHead>> {
        self.aggregate_attempts
            .iter()
            .position(|a| a.id == id)
            .map(|i| attempt_projection(self, i, i + 1 == self.aggregate_attempts.len()))
            .transpose()
    }

    fn has_plan(&self, id: &str) -> Result<bool> {
        Ok(self.plans.iter().any(|p| p.id == id))
    }

    fn previous_run_head(&self) -> Result<Option<RunHead>> {
        match self.runs.len() {
            0 | 1 => Ok(None),
            n => run_projection(self, n - 2, false).map(Some),
        }
    }

    fn review_child(&self, run: &str, id: &str) -> Result<Option<RunHead>> {
        Ok(children_where(self, run, |c| c.id == id, 1)?.pop())
    }
    fn review_child_at(&self, run: &str, n: usize) -> Result<Option<RunHead>> {
        find_run(self, run)
            .and_then(|r| n.checked_sub(1).and_then(|n| r.review_runs.get(n)))
            .map(RunHead::of)
            .transpose()
    }

    fn review_child_by_review_at(&self, run: &str, at: Timestamp) -> Result<Option<RunHead>> {
        Ok(children_where(
            self,
            run,
            |c| c.review.as_ref().is_some_and(|r| r.at == at),
            usize::MAX,
        )?
        .pop())
    }

    fn stronger_source(
        &self,
        run: &str,
        sha: &str,
        exclude: Option<&str>,
    ) -> Result<Option<String>> {
        let stronger = |c: &Run| {
            c.outcome == Some(crate::RunOutcome::Superseded)
                && c.review.as_ref().is_some_and(|r| {
                    r.sha == sha
                        && r.route
                            .as_deref()
                            .is_some_and(|route| route.stronger_request.is_some())
                })
        };
        Ok(children_where(self, run, stronger, 2)?
            .into_iter()
            .map(|c| c.id)
            .find(|id| Some(id.as_str()) != exclude))
    }

    fn plan_review(&self, plan: &str, id: &str) -> Result<Option<PlanReviewHead>> {
        Ok(plan_reviews(self, plan)?.into_iter().find(|r| r.id == id))
    }

    fn plan_review_at(&self, plan: &str, n: usize) -> Result<Option<PlanReviewHead>> {
        Ok(n.checked_sub(1)
            .and_then(|i| plan_reviews(self, plan).ok()?.into_iter().nth(i)))
    }

    fn plan_review_approved_by(
        &self,
        plan: &str,
        reviewer: &str,
    ) -> Result<Option<PlanReviewHead>> {
        Ok(plan_reviews(self, plan)?
            .into_iter()
            .find(|r| r.reviewer == reviewer && r.verdict == Some(crate::Verdict::Approve)))
    }

    fn aggregate_review(&self, attempt: &str, id: &str) -> Result<Option<AggregateReview>> {
        Ok(self
            .aggregate_attempts
            .iter()
            .find(|a| a.id == attempt)
            .and_then(|a| a.reviews.iter().find(|r| r.id == id))
            .cloned())
    }

    fn wait(&self, run: &str, id: &str) -> Result<Option<WaitRecord>> {
        Ok(find_run(self, run)
            .and_then(|r| r.obligations.waits.iter().find(|w| w.request.id == id))
            .cloned())
    }

    fn decision(&self, run: &str, id: &str) -> Result<Option<DecisionRecord>> {
        Ok(find_run(self, run)
            .and_then(|r| r.obligations.decisions.iter().find(|d| d.request.id == id))
            .cloned())
    }

    fn receipt(&self, run: &str, id: &str) -> Result<Option<ResumeReceipt>> {
        let Some(run) = find_run(self, run) else {
            return Ok(None);
        };
        let decisions = run.obligations.decisions.iter().map(|d| d.receipt.as_ref());
        let waits = run.obligations.waits.iter().map(|w| w.receipt.as_ref());
        Ok(decisions
            .chain(waits)
            .flatten()
            .find(|r| r.id == id)
            .cloned())
    }

    fn checkpoint_by_artifact(&self, run: &str, artifact: &str) -> Result<Option<CheckpointMeta>> {
        Ok(checkpoint_metas(self, run)?
            .into_iter()
            .find(|m| m.artifact == artifact))
    }

    fn effect_applied(&self, recipient: &Recipient, seq: u64) -> Result<bool> {
        Ok(inbox_state(self, recipient).is_some_and(|s| s.effects_applied.contains(&seq)))
    }

    fn observe_cursor(
        &self,
        recipient: &Recipient,
        adapter: &str,
        binding: &SessionBinding,
    ) -> Result<Option<SessionCursor>> {
        Ok(inbox_state(self, recipient)
            .and_then(|s| {
                s.observe_cursors
                    .iter()
                    .find(|c| c.adapter == adapter && c.binding == *binding)
            })
            .cloned())
    }

    fn inbox_head(&self, recipient: &Recipient) -> Result<Option<InboxHead>> {
        Ok(inbox_state(self, recipient).map(inbox_head_of))
    }

    fn pending_agent_receipts(&self, container: &ContainerRef) -> Result<Vec<RequestReceipt>> {
        Ok(crate::agent::container_state(self, container)
            .map(|s| {
                s.receipts
                    .iter()
                    .filter(|r| crate::agent::NativeRecord::Receipt((*r).clone()).pending())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }
    fn agent_job(
        &self,
        container: &ContainerRef,
        id: &str,
    ) -> Result<Option<crate::agent::JobRecord>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| s.jobs.iter().find(|j| j.id == id))
            .cloned())
    }
    fn pending_agent_jobs(&self, container: &ContainerRef) -> Result<Vec<crate::agent::JobRecord>> {
        Ok(crate::agent::container_state(self, container)
            .map(|s| s.jobs.iter().filter(|j| j.needs_reap()).cloned().collect())
            .unwrap_or_default())
    }
    fn latest_agent_turn(
        &self,
        container: &ContainerRef,
        generation: u32,
    ) -> Result<Option<AgentTurn>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| {
                s.turns
                    .iter()
                    .filter(|t| t.generation == generation)
                    .max_by_key(|t| t.seq)
            })
            .cloned())
    }

    fn agent_generation(
        &self,
        container: &ContainerRef,
        n: u32,
    ) -> Result<Option<AgentGeneration>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| s.generation(n))
            .cloned())
    }

    fn agent_turn(
        &self,
        container: &ContainerRef,
        generation: u32,
        seq: u64,
    ) -> Result<Option<AgentTurn>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| {
                s.turns
                    .iter()
                    .find(|t| t.generation == generation && t.seq == seq)
            })
            .cloned())
    }

    fn agent_receipt(
        &self,
        container: &ContainerRef,
        request_id: &str,
    ) -> Result<Option<RequestReceipt>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| s.receipts.iter().find(|r| r.request_id == request_id))
            .cloned())
    }

    fn agent_question(&self, container: &ContainerRef, key: &str) -> Result<Option<AgentQuestion>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| s.questions.iter().find(|q| q.key == key))
            .cloned())
    }

    fn pending_agent_questions(&self, container: &ContainerRef) -> Result<Vec<AgentQuestion>> {
        Ok(crate::agent::container_state(self, container)
            .map(|s| {
                s.questions
                    .iter()
                    .filter(|q| q.state.pending())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn agent_wake(
        &self,
        container: &ContainerRef,
        request_id: &str,
    ) -> Result<Option<WakeSubscription>> {
        Ok(crate::agent::container_state(self, container)
            .and_then(|s| s.wakes.iter().find(|w| w.request_id == request_id))
            .cloned())
    }

    fn active_agent_wakes(&self, container: &ContainerRef) -> Result<Vec<WakeSubscription>> {
        use crate::agent::WakeState;
        Ok(crate::agent::container_state(self, container)
            .map(|s| {
                s.wakes
                    .iter()
                    .filter(|w| matches!(w.state, WakeState::Armed | WakeState::PendingStart))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn agent_reports(&self, container: &ContainerRef) -> Result<Vec<AgentReport>> {
        Ok(crate::agent::container_state(self, container)
            .map(|s| s.reports.clone())
            .unwrap_or_default())
    }

    fn pm_receipt(&self, request_id: &str) -> Result<Option<RequestReceipt>> {
        Ok(self
            .agent_pm_receipts
            .iter()
            .find(|r| r.request_id == request_id)
            .cloned())
    }

    fn member_gate(&self, member: &str) -> Result<Option<crate::MemberGateHead>> {
        Ok(crate::task_head_fields(self).member_gates.remove(member))
    }

    fn member_gate_history(&self, member: &str) -> Result<Vec<crate::MemberGateRecord>> {
        Ok(self
            .member_gates
            .get(member)
            .map(|g| g.history.clone())
            .unwrap_or_default())
    }

    fn open_review_children(&self, run: &str) -> Result<Vec<RunHead>> {
        children_where(self, run, |c| c.ended.is_none(), usize::MAX)
    }

    fn pending_plan_reviews(&self, plan: &str) -> Result<Vec<PlanReviewHead>> {
        Ok(plan_reviews(self, plan)?
            .into_iter()
            .filter(|r| r.verdict.is_none())
            .collect())
    }

    fn pending_plan_reviewers(&self) -> Result<Vec<(String, PlanReviewHead)>> {
        self.plans
            .iter()
            .enumerate()
            .filter_map(|(i, plan)| {
                plan.reviews
                    .last()
                    .filter(|review| review.verdict.is_none())
                    .map(|_| (i, plan))
            })
            .map(|(i, plan)| {
                Ok((
                    plan.id.clone(),
                    plan_review_projection(self, i, plan.reviews.len() - 1)?,
                ))
            })
            .collect()
    }

    fn pending_aggregate_reviews(&self, attempt: &str) -> Result<Vec<AggregateReview>> {
        Ok(self
            .aggregate_attempts
            .iter()
            .find(|a| a.id == attempt)
            .map(|a| a.reviews.iter().filter(|r| r.pending()).cloned().collect())
            .unwrap_or_default())
    }

    fn open_obligations(&self, run: &str) -> Result<RunObligations> {
        let Some(found) = find_run(self, run) else {
            return Ok(RunObligations::default());
        };
        Ok(RunObligations {
            slot_wait: found.obligations.slot_wait.clone(),
            waits: found
                .obligations
                .waits
                .iter()
                .filter(|w| w.open(&found.id))
                .cloned()
                .collect(),
            decisions: found
                .obligations
                .decisions
                .iter()
                .filter(|d| d.receipt.is_none())
                .cloned()
                .collect(),
        })
    }

    fn checkpoint_metas_newest_first(
        &self,
        run: &str,
    ) -> Result<Box<dyn Iterator<Item = Result<CheckpointMeta>> + '_>> {
        let mut metas = checkpoint_metas(self, run)?;
        metas.reverse();
        Ok(Box::new(metas.into_iter().map(Ok)))
    }

    fn checkpoint_payload(&self, run: &str, ordinal: u64) -> Result<Option<Checkpoint>> {
        let index = usize::try_from(ordinal)?.checked_sub(1);
        Ok(find_run(self, run)
            .zip(index)
            .and_then(|(run, i)| run.checkpoints.get(i))
            .map(|record| record.checkpoint.clone()))
    }

    fn artifacts_of(&self, run: &str) -> Result<Vec<Artifact>> {
        Ok(self
            .artifacts
            .iter()
            .filter(|a| a.run == run)
            .cloned()
            .collect())
    }

    fn emitted_comments(&self) -> Result<Vec<TaskComment>> {
        Ok(self.comments.clone())
    }

    fn emitted_findings(
        &self,
        source: FindingSource,
        exclude: Option<&str>,
    ) -> Result<Vec<EmittedFinding>> {
        Ok(crate::task_findings(self)
            .into_iter()
            .map(|(_, finding)| finding)
            .filter(|f| f.source == source && Some(f.attempt.as_str()) != exclude)
            .collect())
    }

    fn emitted_obligations(&self, run: &str) -> Result<RunObligations> {
        Ok(find_run(self, run)
            .map(|r| r.obligations.clone())
            .unwrap_or_default())
    }
}
