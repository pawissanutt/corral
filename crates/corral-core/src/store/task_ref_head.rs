//! [`TaskRef`] over heads: every method is the head's own counted read. A mutable head serves
//! its current projections as mutated; its keyed reads observe the view it was read on and are
//! refused once the mutation has queued rows they could miss.

use std::borrow::Cow;

use anyhow::{Result, ensure};
use jiff::Timestamp;

use super::head::TaskHead;
use super::head_mut::TaskHeadMut;
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead};
use crate::agent::{AgentGeneration, AgentTurn, ContainerRef};
use crate::backend::TaskPart;
use crate::inbox::{Recipient, SessionBinding, SessionCursor};
use crate::{
    AggregateReview, Artifact, Checkpoint, CheckpointMeta, DecisionRecord, EmittedFinding,
    FindingSource, HistorySummary, InboxHead, ResumeReceipt, RunObligations, TaskComment, TaskRef,
    WaitRecord,
};

fn count(n: u64, current: bool) -> Result<usize> {
    Ok(usize::try_from(n)? + usize::from(current))
}

macro_rules! keyed_reads {
    ($reads:ident) => {
        fn run_head(&self, id: &str) -> Result<Option<RunHead>> {
            if let Some(current) = self.current_run_head()?.filter(|r| r.id == id) {
                return Ok(Some(current.into_owned()));
            }
            self.$reads()?.run_head(id)
        }
        fn plan_run_head(&self, id: &str) -> Result<Option<PlanRunHead>> {
            if let Some(current) = self.last_plan_run_head()?.filter(|r| r.id == id) {
                return Ok(Some(current.into_owned()));
            }
            self.$reads()?.plan_run_head(id)
        }
        fn plan_head(&self, id: &str) -> Result<Option<PlanHead>> {
            if let Some(current) = self.last_plan_head()?.filter(|p| p.id == id) {
                return Ok(Some(current.into_owned()));
            }
            self.$reads()?.plan_by_id(id)
        }
        fn aggregate_attempt_head(&self, id: &str) -> Result<Option<AttemptHead>> {
            if let Some(current) = self.last_aggregate_attempt_head()?.filter(|a| a.id == id) {
                return Ok(Some(current.into_owned()));
            }
            self.$reads()?.aggregate_attempt_head(id)
        }
        fn has_plan(&self, id: &str) -> Result<bool> {
            Ok(self.plan_head(id)?.is_some())
        }
        fn previous_run_head(&self) -> Result<Option<RunHead>> {
            self.$reads()?.previous_run_head()
        }
        fn review_child(&self, run: &str, id: &str) -> Result<Option<RunHead>> {
            self.$reads()?.review_child(run, id)
        }
        fn review_child_by_review_at(&self, run: &str, at: Timestamp) -> Result<Option<RunHead>> {
            self.$reads()?.review_child_by_review_at(run, at)
        }
        fn stronger_source(
            &self,
            run: &str,
            sha: &str,
            exclude: Option<&str>,
        ) -> Result<Option<String>> {
            self.$reads()?.stronger_source(run, sha, exclude)
        }
        fn plan_review(&self, plan: &str, id: &str) -> Result<Option<PlanReviewHead>> {
            self.$reads()?.plan_review(plan, id)
        }
        fn plan_review_at(&self, plan: &str, n: usize) -> Result<Option<PlanReviewHead>> {
            self.$reads()?.plan_review_at(plan, n)
        }
        fn plan_review_approved_by(
            &self,
            plan: &str,
            reviewer: &str,
        ) -> Result<Option<PlanReviewHead>> {
            self.$reads()?.plan_review_approved_by(plan, reviewer)
        }
        fn aggregate_review(&self, attempt: &str, id: &str) -> Result<Option<AggregateReview>> {
            self.$reads()?.aggregate_review(attempt, id)
        }
        fn wait(&self, run: &str, id: &str) -> Result<Option<WaitRecord>> {
            self.$reads()?.wait(run, id)
        }
        fn decision(&self, run: &str, id: &str) -> Result<Option<DecisionRecord>> {
            self.$reads()?.decision(run, id)
        }
        fn receipt(&self, run: &str, id: &str) -> Result<Option<ResumeReceipt>> {
            self.$reads()?.receipt(run, id)
        }
        fn checkpoint_by_artifact(
            &self,
            run: &str,
            artifact: &str,
        ) -> Result<Option<CheckpointMeta>> {
            self.$reads()?.checkpoint_by_artifact(run, artifact)
        }
        fn effect_applied(&self, recipient: &Recipient, seq: u64) -> Result<bool> {
            self.$reads()?.effect_applied(recipient, seq)
        }
        fn observe_cursor(
            &self,
            recipient: &Recipient,
            adapter: &str,
            binding: &SessionBinding,
        ) -> Result<Option<SessionCursor>> {
            self.$reads()?.observe_cursor(recipient, adapter, binding)
        }
        fn inbox_head(&self, recipient: &Recipient) -> Result<Option<InboxHead>> {
            self.$reads()?.inbox_head(recipient)
        }
        fn agent_generation(
            &self,
            container: &ContainerRef,
            n: u32,
        ) -> Result<Option<AgentGeneration>> {
            self.$reads()?.agent_generation(container, n)
        }
        fn agent_turn(
            &self,
            container: &ContainerRef,
            generation: u32,
            seq: u64,
        ) -> Result<Option<AgentTurn>> {
            self.$reads()?.agent_turn(container, generation, seq)
        }
        fn open_review_children(&self, run: &str) -> Result<Vec<RunHead>> {
            self.$reads()?.open_review_children(run)
        }
        fn pending_plan_reviews(&self, plan: &str) -> Result<Vec<PlanReviewHead>> {
            self.$reads()?.pending_plan_reviews(plan)
        }
        fn pending_aggregate_reviews(&self, attempt: &str) -> Result<Vec<AggregateReview>> {
            self.$reads()?.pending_aggregate_reviews(attempt)
        }
        fn open_obligations(&self, run: &str) -> Result<RunObligations> {
            self.$reads()?.open_obligations(run)
        }
        fn checkpoint_metas_newest_first(
            &self,
            run: &str,
        ) -> Result<Box<dyn Iterator<Item = Result<CheckpointMeta>> + '_>> {
            Ok(Box::new(self.$reads()?.checkpoint_metas_newest_first(run)?))
        }
        fn checkpoint_payload(&self, run: &str, ordinal: u64) -> Result<Option<Checkpoint>> {
            self.$reads()?.checkpoint_payload(run, ordinal)
        }
        fn artifacts_of(&self, run: &str) -> Result<Vec<Artifact>> {
            self.$reads()?.artifacts_of(run)
        }
        fn emitted_comments(&self) -> Result<Vec<TaskComment>> {
            self.$reads()?.emitted_comments()
        }
        fn emitted_findings(
            &self,
            source: FindingSource,
            exclude: Option<&str>,
        ) -> Result<Vec<EmittedFinding>> {
            self.$reads()?.emitted_findings(source, exclude)
        }
        fn emitted_obligations(&self, run: &str) -> Result<RunObligations> {
            self.$reads()?.emitted_obligations(run)
        }
    };
}

impl TaskHead {
    fn reads(&self) -> Result<&TaskHead> {
        Ok(self)
    }
}

impl TaskHeadMut {
    fn reads(&self) -> Result<&TaskHead> {
        ensure!(
            self.demoted.is_empty()
                && self
                    .rows
                    .keys()
                    .all(|(part, _)| matches!(part, TaskPart::PaneRef | TaskPart::Finding)),
            "a keyed read after a queued row write in the same head mutation is not supported"
        );
        Ok(&self.head)
    }
}

macro_rules! projections {
    ($head:ident) => {
        fn current_run_head(&self) -> Result<Option<Cow<'_, RunHead>>> {
            Ok(self.$head().current_run.as_ref().map(Cow::Borrowed))
        }
        fn last_plan_run_head(&self) -> Result<Option<Cow<'_, PlanRunHead>>> {
            Ok(self.$head().last_plan_run.as_ref().map(Cow::Borrowed))
        }
        fn last_plan_head(&self) -> Result<Option<Cow<'_, PlanHead>>> {
            Ok(self.$head().last_plan.as_ref().map(Cow::Borrowed))
        }
        fn last_aggregate_attempt_head(&self) -> Result<Option<Cow<'_, AttemptHead>>> {
            Ok(self
                .$head()
                .last_aggregate_attempt
                .as_ref()
                .map(Cow::Borrowed))
        }
        fn history_summary(&self) -> Result<Cow<'_, HistorySummary>> {
            Ok(Cow::Borrowed(&self.$head().summary))
        }
        fn runs_len(&self) -> Result<usize> {
            let head = self.$head();
            count(head.summary.runs, head.current_run.is_some())
        }
        fn plan_runs_len(&self) -> Result<usize> {
            let head = self.$head();
            count(head.summary.plan_runs, head.last_plan_run.is_some())
        }
        fn plans_len(&self) -> Result<usize> {
            let head = self.$head();
            count(head.summary.plans, head.last_plan.is_some())
        }
        fn aggregate_attempts_len(&self) -> Result<usize> {
            let head = self.$head();
            count(
                head.summary.aggregate_attempts,
                head.last_aggregate_attempt.is_some(),
            )
        }
        fn comments_len(&self) -> Result<usize> {
            Ok(usize::try_from(self.$head().summary.comments)?)
        }
        fn head_fields(&self) -> Result<Cow<'_, crate::TaskHeadFields>> {
            Ok(Cow::Borrowed(&self.$head().task))
        }
        fn contract_revision(&self) -> Result<String> {
            Ok(self.$head().summary.operating_contract_revision.clone())
        }
    };
}

impl TaskHead {
    fn as_head(&self) -> &TaskHead {
        self
    }
}

impl TaskRef for TaskHead {
    projections!(as_head);
    keyed_reads!(reads);
}

impl TaskRef for TaskHeadMut {
    projections!(head);
    keyed_reads!(reads);
}

/// [`TaskRef::shell_task`]: the head fields and the current containers as a full task.
pub fn shell_of<T: TaskRef + ?Sized>(task: &T) -> Result<crate::Task> {
    fn run(head: &RunHead) -> Result<crate::Run> {
        let mut value: crate::Run = serde_json::from_value(serde_json::to_value(head)?)?;
        if let Some(child) = head.current_child.as_deref() {
            value.review_runs.push(run(child)?);
        }
        Ok(value)
    }
    let mut shell = super::head_api::shell_task(task.head_fields()?.as_ref())?;
    if let Some(head) = task.current_run_head()? {
        shell.runs.push(run(&head)?);
    }
    if let Some(head) = task.last_plan_run_head()? {
        shell
            .plan_runs
            .push(serde_json::from_value(serde_json::to_value(&*head)?)?);
    }
    if let Some(head) = task.last_plan_head()? {
        let mut plan: crate::PlanRecord = serde_json::from_value(serde_json::to_value(&*head)?)?;
        if let Some(review) = &head.current_review {
            plan.reviews
                .push(serde_json::from_value(serde_json::to_value(review)?)?);
        }
        shell.plans.push(plan);
    }
    if let Some(head) = task.last_aggregate_attempt_head()? {
        let mut attempt: crate::AggregateAttempt =
            serde_json::from_value(serde_json::to_value(&*head)?)?;
        attempt.reviews.extend(head.current_review.clone());
        shell.aggregate_attempts.push(attempt);
    }
    Ok(shell)
}
