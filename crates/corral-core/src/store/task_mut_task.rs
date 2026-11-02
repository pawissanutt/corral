//! [`TaskMut`] over the full task's vectors, with the refusals of the head form: a keyed edit
//! may change neither a row's identity or ordering fields nor what only another mutation keeps
//! consistent, and child mutations act on the current container only.

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::task_ref_task::{
    attempt_projection, child_projection, inbox_head_of, plan_projection, plan_review_projection,
    plan_run_projection, run_projection,
};
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead};
use crate::inbox::{InboxState, Recipient, SessionCursor};
use crate::{
    AggregateAttempt, AggregateReview, ApprovedScopeRevision, CheckpointRecord, DecisionRecord,
    InboxHead, PlanRecord, PlanReview, PlanRun, ProfileSwitch, Run, Task, TaskComment,
    TaskHeadFields, TaskMut, Usage, UsageEntry, WaitRecord,
};

pub(crate) fn check_run_edit(before: &RunHead, after: &RunHead) -> Result<()> {
    ensure!(
        before.id == after.id && before.started == after.started && before.ended == after.ended,
        "a keyed run edit may not change its id, started or ended"
    );
    ensure!(
        before.usage == after.usage
            && before.inbox == after.inbox
            && before.agent_host == after.agent_host
            && before.current_child == after.current_child
            && before.facts == after.facts,
        "a keyed run edit may not change usage, inbox, agent records, children or facts"
    );
    Ok(())
}

pub(crate) fn check_plan_run_edit(before: &PlanRunHead, after: &PlanRunHead) -> Result<()> {
    ensure!(
        before.id == after.id && before.started == after.started && before.ended == after.ended,
        "a keyed plan run edit may not change its id, started or ended"
    );
    ensure!(
        before.inbox == after.inbox && before.facts == after.facts,
        "a keyed plan run edit may not change its inbox or facts"
    );
    Ok(())
}

pub(crate) fn check_plan_edit(before: &PlanHead, after: &PlanHead) -> Result<()> {
    ensure!(
        before.id == after.id,
        "a keyed plan edit may not change its id"
    );
    ensure!(
        before.current_review == after.current_review && before.facts == after.facts,
        "a keyed plan edit may not change its reviews or facts"
    );
    Ok(())
}

pub(crate) fn check_attempt_edit(before: &AttemptHead, after: &AttemptHead) -> Result<()> {
    ensure!(
        before.id == after.id && before.started == after.started,
        "a keyed aggregate attempt edit may not change its id or started"
    );
    ensure!(
        before.current_review == after.current_review && before.facts == after.facts,
        "a keyed aggregate attempt edit may not change its reviews or facts"
    );
    Ok(())
}

/// Writes the projection fields `after` changed relative to `before` into `full`.
fn overlay<F: Serialize + DeserializeOwned, P: Serialize>(
    full: &mut F,
    before: &P,
    after: &P,
) -> Result<()> {
    let (Value::Object(before), Value::Object(after)) =
        (serde_json::to_value(before)?, serde_json::to_value(after)?)
    else {
        bail!("a projection is not an object");
    };
    let Value::Object(mut doc) = serde_json::to_value(&*full)? else {
        bail!("a container is not an object");
    };
    for key in before.keys().chain(after.keys()) {
        if before.get(key) == after.get(key) {
            continue;
        }
        match after.get(key) {
            Some(value) => doc.insert(key.clone(), value.clone()),
            None => doc.remove(key),
        };
    }
    *full = serde_json::from_value(Value::Object(doc))?;
    Ok(())
}

fn current_run<'a>(task: &'a mut Task, run: &str) -> Result<&'a mut Run> {
    let id = task.id.clone();
    task.runs
        .last_mut()
        .filter(|r| r.id == run)
        .with_context(|| format!("run {run} is not the current run of {id}"))
}

fn current_plan<'a>(task: &'a mut Task, plan: &str) -> Result<&'a mut PlanRecord> {
    let id = task.id.clone();
    task.plans
        .last_mut()
        .filter(|p| p.id == plan)
        .with_context(|| format!("plan {plan} is not the current plan of {id}"))
}

fn current_attempt<'a>(task: &'a mut Task, attempt: &str) -> Result<&'a mut AggregateAttempt> {
    let id = task.id.clone();
    task.aggregate_attempts
        .last_mut()
        .filter(|a| a.id == attempt)
        .with_context(|| format!("aggregate attempt {attempt} is not current on {id}"))
}

fn usage_owner<'a>(run: &'a mut Run, child: Option<&str>) -> Result<&'a mut Run> {
    match child {
        None => Ok(run),
        Some(child) => run
            .review_runs
            .iter_mut()
            .find(|c| c.id == child)
            .with_context(|| format!("review child {child} not found")),
    }
}

fn inbox_state_mut<'a>(task: &'a mut Task, recipient: &Recipient) -> Result<&'a mut InboxState> {
    let missing = || anyhow::anyhow!("the recipient has no inbox on this task");
    match recipient {
        Recipient::Run { run, .. } => Ok(&mut task
            .runs
            .iter_mut()
            .find(|r| r.id == *run)
            .ok_or_else(missing)?
            .inbox),
        Recipient::ReviewRun { run, review, .. } => Ok(&mut task
            .runs
            .iter_mut()
            .find(|r| r.id == *run)
            .ok_or_else(missing)?
            .review_runs
            .iter_mut()
            .find(|c| c.id == *review)
            .ok_or_else(missing)?
            .inbox),
        Recipient::PlanRun { plan_run, .. } => Ok(&mut task
            .plan_runs
            .iter_mut()
            .find(|r| r.id == *plan_run)
            .ok_or_else(missing)?
            .inbox),
        Recipient::PlanReview { plan, review, .. } => Ok(&mut task
            .plans
            .iter_mut()
            .find(|p| p.id == *plan)
            .ok_or_else(missing)?
            .reviews
            .iter_mut()
            .find(|r| r.id == *review)
            .ok_or_else(missing)?
            .inbox),
        Recipient::Pm | Recipient::Assistant { .. } => Err(missing()),
    }
}

impl TaskMut for Task {
    fn init_agent_head(
        &mut self,
        container: &crate::agent::ContainerRef,
        head: crate::AgentHostHead,
    ) -> Result<()> {
        ensure!(
            head.records == Default::default(),
            "a new native head cannot claim existing records"
        );
        let slot = crate::agent::container_host_mut(self, container)
            .context("native container missing")?;
        ensure!(slot.is_none(), "corral-pi host already initialized");
        *slot = Some(Box::new(serde_json::from_value(serde_json::to_value(
            head,
        )?)?));
        Ok(())
    }
    fn edit_agent_head(
        &mut self,
        container: &crate::agent::ContainerRef,
        edit: impl FnOnce(&mut crate::AgentHostHead) -> Result<()>,
    ) -> Result<()> {
        let state =
            crate::agent::container_state_mut(self, container).context("corral-pi host missing")?;
        let mut head = crate::AgentHostHead::of(state);
        let counts = head.records.clone();
        edit(&mut head)?;
        ensure!(
            head.records == counts,
            "native append positions are store-owned"
        );
        state.version = head.version;
        state.binding = head.binding;
        state.todo = head.todo;
        state.question_hold = head.question_hold;
        Ok(())
    }
    fn put_agent_record(
        &mut self,
        container: &crate::agent::ContainerRef,
        record: crate::agent::NativeRecord,
    ) -> Result<bool> {
        Ok(record.put(
            crate::agent::container_state_mut(self, container).context("corral-pi host missing")?,
        ))
    }

    fn edit_head_fields(
        &mut self,
        edit: impl FnOnce(&mut TaskHeadFields) -> Result<()>,
    ) -> Result<()> {
        let mut fields = crate::task_head_fields(self);
        edit(&mut fields)?;
        crate::task_ref::set_task_head_fields(self, fields)
    }

    fn start_run(&mut self, run: Run) -> Result<()> {
        self.runs.push(run);
        Ok(())
    }

    fn start_plan_run(&mut self, run: PlanRun) -> Result<()> {
        self.plan_runs.push(run);
        Ok(())
    }

    fn push_plan(&mut self, plan: PlanRecord) -> Result<()> {
        self.plans.push(plan);
        Ok(())
    }

    fn push_aggregate_attempt(&mut self, attempt: AggregateAttempt) -> Result<()> {
        self.aggregate_attempts.push(attempt);
        Ok(())
    }

    fn add_comment(&mut self, comment: TaskComment) -> Result<()> {
        self.comments.push(comment);
        Ok(())
    }

    fn push_scope_revision(&mut self, revision: ApprovedScopeRevision) -> Result<()> {
        self.approved_scope_history.push(revision);
        Ok(())
    }

    fn edit_run(&mut self, id: &str, edit: impl FnOnce(&mut RunHead) -> Result<()>) -> Result<()> {
        let i = self
            .runs
            .iter()
            .position(|r| r.id == id)
            .with_context(|| format!("run {id} not found"))?;
        let before = run_projection(self, i, i + 1 == self.runs.len())?;
        let mut after = before.clone();
        edit(&mut after)?;
        check_run_edit(&before, &after)?;
        overlay(&mut self.runs[i], &before, &after)
    }

    fn edit_plan_run(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut PlanRunHead) -> Result<()>,
    ) -> Result<()> {
        let i = self
            .plan_runs
            .iter()
            .position(|r| r.id == id)
            .with_context(|| format!("plan run {id} not found"))?;
        let before = plan_run_projection(self, i)?;
        let mut after = before.clone();
        edit(&mut after)?;
        check_plan_run_edit(&before, &after)?;
        overlay(&mut self.plan_runs[i], &before, &after)
    }

    fn edit_plan(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut PlanHead) -> Result<()>,
    ) -> Result<()> {
        let i = self
            .plans
            .iter()
            .position(|p| p.id == id)
            .with_context(|| format!("plan {id} not found"))?;
        let before = plan_projection(self, i, i + 1 == self.plans.len())?;
        let mut after = before.clone();
        edit(&mut after)?;
        check_plan_edit(&before, &after)?;
        overlay(&mut self.plans[i], &before, &after)
    }

    fn edit_aggregate_attempt(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut AttemptHead) -> Result<()>,
    ) -> Result<()> {
        let i = self
            .aggregate_attempts
            .iter()
            .position(|a| a.id == id)
            .with_context(|| format!("aggregate attempt {id} not found"))?;
        let last = i + 1 == self.aggregate_attempts.len();
        let before = attempt_projection(self, i, last)?;
        let mut after = before.clone();
        edit(&mut after)?;
        check_attempt_edit(&before, &after)?;
        overlay(&mut self.aggregate_attempts[i], &before, &after)
    }

    fn inbox_state_mut<R>(
        &mut self,
        recipient: &Recipient,
        edit: impl FnOnce(&mut InboxHead) -> Result<R>,
    ) -> Result<R> {
        let state = inbox_state_mut(self, recipient)?;
        let mut head = inbox_head_of(state);
        let result = edit(&mut head)?;
        state.next_seq = head.next_seq;
        state.next_envelope = head.next_envelope;
        state.open_envelope = head.open_envelope;
        state.replacement_envelope = head.replacement_envelope;
        state.binding = head.binding;
        state.observe_cursor = head.observe_cursor;
        state.settled = head.settled;
        Ok(result)
    }

    fn push_review_child(&mut self, run: &str, child: Run) -> Result<()> {
        let current = current_run(self, run)?;
        ensure!(
            current.review_runs.iter().all(|c| c.id != child.id),
            "review child {} already exists on {run}",
            child.id
        );
        current.review_runs.push(child);
        Ok(())
    }

    fn edit_review_child(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut RunHead) -> Result<()>,
    ) -> Result<()> {
        current_run(self, run)?;
        let i = self.runs.len() - 1;
        let c = self.runs[i]
            .review_runs
            .iter()
            .position(|c| c.id == id)
            .with_context(|| format!("review child {id} not found on {run}"))?;
        let before = child_projection(self, i, c)?;
        let mut after = before.clone();
        edit(&mut after)?;
        check_run_edit(&before, &after)?;
        overlay(&mut self.runs[i].review_runs[c], &before, &after)
    }

    fn push_plan_review(&mut self, plan: &str, review: PlanReview) -> Result<()> {
        let current = current_plan(self, plan)?;
        ensure!(
            current.reviews.iter().all(|r| r.id != review.id),
            "plan review {} already exists on {plan}",
            review.id
        );
        current.reviews.push(review);
        Ok(())
    }

    fn edit_plan_review(
        &mut self,
        plan: &str,
        id: &str,
        edit: impl FnOnce(&mut PlanReviewHead) -> Result<()>,
    ) -> Result<()> {
        current_plan(self, plan)?;
        let i = self.plans.len() - 1;
        let r = self.plans[i]
            .reviews
            .iter()
            .position(|r| r.id == id)
            .with_context(|| format!("plan review {id} not found on {plan}"))?;
        let before = plan_review_projection(self, i, r)?;
        let mut after = before.clone();
        edit(&mut after)?;
        super::head_child_edit::check_plan_review_edit(&before, &after)?;
        overlay(&mut self.plans[i].reviews[r], &before, &after)
    }

    fn pop_current_plan_review(&mut self, plan: &str) -> Result<()> {
        current_plan(self, plan)?
            .reviews
            .pop()
            .context("the plan has no current review")?;
        Ok(())
    }

    fn terminalize_pending_plan_reviews(&mut self, plan: &str, reason: &str) -> Result<()> {
        for review in &mut current_plan(self, plan)?.reviews {
            if review.verdict.is_none() {
                review.verdict = Some(crate::Verdict::Escalate);
                review.notes = Some(reason.into());
                review.pending_reply = None;
                review.at = crate::clock::now();
            }
        }
        Ok(())
    }

    fn push_aggregate_review(&mut self, attempt: &str, review: AggregateReview) -> Result<()> {
        let current = current_attempt(self, attempt)?;
        ensure!(
            current.reviews.iter().all(|r| r.id != review.id),
            "aggregate review {} already exists on {attempt}",
            review.id
        );
        current.reviews.push(review);
        Ok(())
    }

    fn edit_aggregate_review(
        &mut self,
        attempt: &str,
        id: &str,
        edit: impl FnOnce(&mut AggregateReview) -> Result<()>,
    ) -> Result<()> {
        let review = current_attempt(self, attempt)?
            .reviews
            .iter_mut()
            .find(|r| r.id == id)
            .with_context(|| format!("aggregate review {id} not found on {attempt}"))?;
        let mut after = review.clone();
        edit(&mut after)?;
        ensure!(
            after.id == review.id,
            "an aggregate review edit may not change its id"
        );
        *review = after;
        Ok(())
    }

    fn supersede_pending_aggregate_reviews(&mut self, attempt: &str, by: &str) -> Result<()> {
        for review in &mut current_attempt(self, attempt)?.reviews {
            if review.pending() {
                review.abandoned = Some(format!("superseded by {by}"));
            }
        }
        Ok(())
    }

    fn push_checkpoint(&mut self, run: &str, record: CheckpointRecord) -> Result<()> {
        current_run(self, run)?.checkpoints.push(record);
        Ok(())
    }

    fn restore_checkpoints(&mut self, run: &str, records: Vec<CheckpointRecord>) -> Result<()> {
        ensure!(records.len() <= 2, "a run restores at most two checkpoints");
        current_run(self, run)?.checkpoints.extend(records);
        Ok(())
    }

    fn push_switch(&mut self, run: &str, switch: ProfileSwitch) -> Result<()> {
        current_run(self, run)?.switches.push(switch);
        Ok(())
    }

    fn push_wait(&mut self, run: &str, wait: WaitRecord) -> Result<()> {
        let current = current_run(self, run)?;
        ensure_new_obligation(current, &wait.request.id)?;
        current.obligations.waits.push(wait);
        Ok(())
    }

    fn push_decision(&mut self, run: &str, decision: DecisionRecord) -> Result<()> {
        let current = current_run(self, run)?;
        ensure_new_obligation(current, &decision.request.id)?;
        current.obligations.decisions.push(decision);
        Ok(())
    }

    fn edit_run_wait(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut WaitRecord) -> Result<()>,
    ) -> Result<()> {
        let target = self
            .runs
            .iter_mut()
            .find(|target| target.id == run)
            .with_context(|| format!("run {run} not found"))?;
        let wait = target
            .obligations
            .waits
            .iter_mut()
            .find(|wait| wait.request.id == id)
            .with_context(|| format!("wait {id} not found"))?;
        let mut after = wait.clone();
        edit(&mut after)?;
        ensure!(
            after.request.id == wait.request.id,
            "a wait edit may not change its id"
        );
        *wait = after;
        Ok(())
    }

    fn edit_wait(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut WaitRecord) -> Result<()>,
    ) -> Result<()> {
        let wait = current_run(self, run)?
            .obligations
            .waits
            .iter_mut()
            .find(|w| w.request.id == id)
            .with_context(|| format!("wait {id} not found"))?;
        let mut after = wait.clone();
        edit(&mut after)?;
        ensure!(
            after.request.id == wait.request.id,
            "a wait edit may not change its id"
        );
        *wait = after;
        Ok(())
    }

    fn edit_decision(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut DecisionRecord) -> Result<()>,
    ) -> Result<()> {
        let decision = current_run(self, run)?
            .obligations
            .decisions
            .iter_mut()
            .find(|d| d.request.id == id)
            .with_context(|| format!("decision {id} not found"))?;
        let mut after = decision.clone();
        edit(&mut after)?;
        ensure!(
            after.request.id == decision.request.id,
            "a decision edit may not change its id"
        );
        *decision = after;
        Ok(())
    }

    fn mark_effect_applied(&mut self, recipient: &Recipient, effect: u64) -> Result<()> {
        inbox_state_mut(self, recipient)?
            .effects_applied
            .insert(effect);
        Ok(())
    }

    fn set_observe_cursor(&mut self, recipient: &Recipient, cursor: SessionCursor) -> Result<()> {
        let cursors = &mut inbox_state_mut(self, recipient)?.observe_cursors;
        match cursors
            .iter_mut()
            .find(|c| c.adapter == cursor.adapter && c.binding == cursor.binding)
        {
            Some(existing) => *existing = cursor,
            None => cursors.push(cursor),
        }
        Ok(())
    }

    fn append_usage_entries(
        &mut self,
        run: &str,
        child: Option<&str>,
        entries: Vec<UsageEntry>,
    ) -> Result<()> {
        let owner = usage_owner(current_run(self, run)?, child)?;
        let mut all = owner.usage.take().map(|u| u.entries).unwrap_or_default();
        all.extend(entries);
        owner.usage = Some(Usage::from_entries(all));
        Ok(())
    }

    fn set_usage(&mut self, run: &str, child: Option<&str>, usage: Usage) -> Result<()> {
        usage_owner(current_run(self, run)?, child)?.usage = Some(usage);
        Ok(())
    }
}

fn ensure_new_obligation(run: &Run, id: &str) -> Result<()> {
    let waits = run.obligations.waits.iter().map(|w| &w.request.id);
    let decisions = run.obligations.decisions.iter().map(|d| &d.request.id);
    ensure!(
        !waits.chain(decisions).any(|existing| existing == id),
        "obligation {id} already exists"
    );
    Ok(())
}
