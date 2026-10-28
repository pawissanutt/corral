//! [`TaskMut`] over a mutable head: keyed edits of current containers change their projections
//! in place; edits of earlier ones write that one row and adjust the summary by its delta.

use anyhow::{Context, Result};

use super::codec::seq;
use super::head::{last_ordinal, projection};
use super::head_mut::{
    TaskHeadMut, add_count, add_panes, attempt_row, plan_row, plan_run_row, run_own_panes, run_row,
};
use super::task_mut_task::{
    check_attempt_edit, check_plan_edit, check_plan_run_edit, check_run_edit,
};
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead};
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::container_facts::herdr_panes;
use crate::inbox::{Recipient, SessionCursor};
use crate::work::Counter;
use crate::{
    AggregateAttempt, AggregateReview, ApprovedScopeRevision, CheckpointRecord, DecisionRecord,
    InboxHead, PlanRecord, PlanReview, PlanRun, ProfileSwitch, Run, TaskComment, TaskHeadFields,
    TaskMut, Usage, UsageEntry, WaitRecord,
};

impl TaskHeadMut {
    fn history_container(&self, part: TaskPart, id: &str) -> Result<(usize, HistoryRow)> {
        let row = self
            .rows_now(
                &RowQuery::part(part).with_index(format!("id:{id}")),
                Counter::HistoryRowDecodes,
            )?
            .into_iter()
            .next()
            .with_context(|| format!("{part:?} {id} not found on {}", self.head.task.id))?;
        Ok((last_ordinal(&row.key)?, row))
    }

    fn edit_history_run(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut RunHead) -> Result<()>,
    ) -> Result<()> {
        let (s, row) = self.history_container(TaskPart::Run, id)?;
        let (mut before, facts): (RunHead, _) = projection(&row.doc)?;
        before.facts = facts;
        let mut after = before.clone();
        edit(&mut after)?;
        check_run_edit(&before, &after)?;
        if after.harness != before.harness {
            for (harness, sign) in [(&before.harness, -1), (&after.harness, 1)] {
                if let Some(h) = harness {
                    add_count(&mut after.facts.harnesses, h, sign);
                }
            }
        }
        let (old_panes, new_panes) = (
            run_own_panes(s, super::codec::SELF, &before),
            run_own_panes(s, super::codec::SELF, &after),
        );
        for map in [&mut after.facts.pane_refs, &mut self.head.summary.pane_refs] {
            add_panes(map, &old_panes, -1);
            add_panes(map, &new_panes, 1);
        }
        self.write_panes(&old_panes, &new_panes);
        self.head.summary.adjust_run_head(&before, &after);
        self.put(run_row(s, super::codec::SELF, &after, false)?);
        Ok(())
    }

    fn edit_history_plan_run(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut PlanRunHead) -> Result<()>,
    ) -> Result<()> {
        let (s, row) = self.history_container(TaskPart::PlanRun, id)?;
        let (mut before, facts): (PlanRunHead, _) = projection(&row.doc)?;
        before.facts = facts;
        let mut after = before.clone();
        edit(&mut after)?;
        check_plan_run_edit(&before, &after)?;
        let owner = format!("plan_run:{}", seq(s));
        let old_panes = herdr_panes(&owner, "plan_run", before.herdr.as_ref());
        let new_panes = herdr_panes(&owner, "plan_run", after.herdr.as_ref());
        for map in [&mut after.facts.pane_refs, &mut self.head.summary.pane_refs] {
            add_panes(map, &old_panes, -1);
            add_panes(map, &new_panes, 1);
        }
        self.write_panes(&old_panes, &new_panes);
        self.put(plan_run_row(s, &after, false)?);
        Ok(())
    }
}

/// Runs one trait mutation, then [`TaskHeadMut::sync`], so the next `TaskRef` read of the
/// same operation sees facts that agree with the edit.
macro_rules! synced {
    ($self:ident, $mutation:expr) => {{
        let result = $mutation;
        if result.is_ok() {
            $self.sync()?;
        }
        result
    }};
}

impl TaskMut for TaskHeadMut {
    fn edit_head_fields(
        &mut self,
        edit: impl FnOnce(&mut TaskHeadFields) -> Result<()>,
    ) -> Result<()> {
        synced!(
            self,
            (|| -> Result<()> {
                let mut fields = self.head.task.clone();
                edit(&mut fields)?;
                anyhow::ensure!(
                    fields.member_gates == self.head.task.member_gates,
                    "member gates change only through a member-gate amendment"
                );
                self.head.task = fields;
                Ok(())
            })()
        )
    }

    fn start_run(&mut self, run: Run) -> Result<()> {
        synced!(self, TaskHeadMut::start_run(self, run))
    }

    fn start_plan_run(&mut self, run: PlanRun) -> Result<()> {
        synced!(self, TaskHeadMut::start_plan_run(self, run))
    }

    fn push_plan(&mut self, plan: PlanRecord) -> Result<()> {
        synced!(self, TaskHeadMut::push_plan(self, plan))
    }

    fn push_aggregate_attempt(&mut self, attempt: AggregateAttempt) -> Result<()> {
        synced!(self, TaskHeadMut::push_aggregate_attempt(self, attempt))
    }

    fn add_comment(&mut self, comment: TaskComment) -> Result<()> {
        synced!(self, TaskHeadMut::add_comment(self, comment))
    }

    fn push_scope_revision(&mut self, revision: ApprovedScopeRevision) -> Result<()> {
        synced!(self, TaskHeadMut::push_scope_revision(self, revision))
    }

    fn edit_run(&mut self, id: &str, edit: impl FnOnce(&mut RunHead) -> Result<()>) -> Result<()> {
        synced!(
            self,
            (|| -> Result<()> {
                if let Some(current) = self.head.current_run.as_mut().filter(|r| r.id == id) {
                    let mut after = current.clone();
                    edit(&mut after)?;
                    check_run_edit(current, &after)?;
                    *current = after;
                    return Ok(());
                }
                self.edit_history_run(id, edit)
            })()
        )
    }

    fn edit_plan_run(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut PlanRunHead) -> Result<()>,
    ) -> Result<()> {
        synced!(
            self,
            (|| -> Result<()> {
                if let Some(current) = self.head.last_plan_run.as_mut().filter(|r| r.id == id) {
                    let mut after = current.clone();
                    edit(&mut after)?;
                    check_plan_run_edit(current, &after)?;
                    *current = after;
                    return Ok(());
                }
                self.edit_history_plan_run(id, edit)
            })()
        )
    }

    fn edit_plan(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut PlanHead) -> Result<()>,
    ) -> Result<()> {
        synced!(
            self,
            (|| -> Result<()> {
                if let Some(current) = self.head.last_plan.as_mut().filter(|p| p.id == id) {
                    let mut after = current.clone();
                    edit(&mut after)?;
                    check_plan_edit(current, &after)?;
                    *current = after;
                    return Ok(());
                }
                let (s, row) = self.history_container(TaskPart::Plan, id)?;
                let (mut before, facts): (PlanHead, _) = projection(&row.doc)?;
                before.facts = facts;
                let mut after = before.clone();
                edit(&mut after)?;
                check_plan_edit(&before, &after)?;
                self.put(plan_row(s, &after, false)?);
                Ok(())
            })()
        )
    }

    fn edit_aggregate_attempt(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut AttemptHead) -> Result<()>,
    ) -> Result<()> {
        synced!(
            self,
            (|| -> Result<()> {
                if let Some(current) = self
                    .head
                    .last_aggregate_attempt
                    .as_mut()
                    .filter(|a| a.id == id)
                {
                    let mut after = current.clone();
                    edit(&mut after)?;
                    check_attempt_edit(current, &after)?;
                    *current = after;
                    return Ok(());
                }
                let (s, row) = self.history_container(TaskPart::AggregateAttempt, id)?;
                let (mut before, facts): (AttemptHead, _) = projection(&row.doc)?;
                before.facts = facts;
                let mut after = before.clone();
                edit(&mut after)?;
                check_attempt_edit(&before, &after)?;
                self.put(attempt_row(s, &after, false)?);
                Ok(())
            })()
        )
    }

    fn inbox_state_mut<R>(
        &mut self,
        recipient: &Recipient,
        edit: impl FnOnce(&mut InboxHead) -> Result<R>,
    ) -> Result<R> {
        synced!(
            self,
            (|| -> Result<R> {
                let head = &mut self.head;
                let inbox = match recipient {
                    Recipient::Run { run, .. } => head
                        .current_run
                        .as_mut()
                        .filter(|r| r.id == *run)
                        .map(|r| &mut r.inbox),
                    Recipient::ReviewRun { run, review, .. } => head
                        .current_run
                        .as_mut()
                        .filter(|r| r.id == *run)
                        .and_then(|r| r.current_child.as_deref_mut())
                        .filter(|c| c.id == *review)
                        .map(|c| &mut c.inbox),
                    Recipient::PlanRun { plan_run, .. } => head
                        .last_plan_run
                        .as_mut()
                        .filter(|r| r.id == *plan_run)
                        .map(|r| &mut r.inbox),
                    Recipient::PlanReview { plan, review, .. } => head
                        .last_plan
                        .as_mut()
                        .filter(|p| p.id == *plan)
                        .and_then(|p| p.current_review.as_mut())
                        .filter(|r| r.id == *review)
                        .map(|r| &mut r.inbox),
                    Recipient::Pm => None,
                };
                edit(
                    inbox.context(
                        "the recipient's inbox is not on a current container of this head",
                    )?,
                )
            })()
        )
    }

    fn push_review_child(&mut self, run: &str, child: Run) -> Result<()> {
        synced!(self, TaskHeadMut::push_review_child(self, run, child))
    }

    fn edit_review_child(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut RunHead) -> Result<()>,
    ) -> Result<()> {
        synced!(self, self.edit_review_child_head(run, id, edit))
    }

    fn push_plan_review(&mut self, plan: &str, review: PlanReview) -> Result<()> {
        synced!(self, TaskHeadMut::push_plan_review(self, plan, review))
    }

    fn edit_plan_review(
        &mut self,
        plan: &str,
        id: &str,
        edit: impl FnOnce(&mut PlanReviewHead) -> Result<()>,
    ) -> Result<()> {
        synced!(self, self.edit_plan_review_head(plan, id, edit))
    }

    fn pop_current_plan_review(&mut self, plan: &str) -> Result<()> {
        synced!(self, TaskHeadMut::pop_current_plan_review(self, plan))
    }

    fn terminalize_pending_plan_reviews(&mut self, plan: &str, reason: &str) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::terminalize_pending_plan_reviews(self, plan, reason)
        )
    }

    fn push_aggregate_review(&mut self, attempt: &str, review: AggregateReview) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::push_aggregate_review(self, attempt, review)
        )
    }

    fn edit_aggregate_review(
        &mut self,
        attempt: &str,
        id: &str,
        edit: impl FnOnce(&mut AggregateReview) -> Result<()>,
    ) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::edit_aggregate_review(self, attempt, id, edit)
        )
    }

    fn supersede_pending_aggregate_reviews(&mut self, attempt: &str, by: &str) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::supersede_pending_aggregate_reviews(self, attempt, by)
        )
    }

    fn push_checkpoint(&mut self, run: &str, record: CheckpointRecord) -> Result<()> {
        synced!(self, TaskHeadMut::push_checkpoint(self, run, record))
    }

    fn restore_checkpoints(&mut self, run: &str, records: Vec<CheckpointRecord>) -> Result<()> {
        synced!(self, TaskHeadMut::restore_checkpoints(self, run, records))
    }

    fn push_switch(&mut self, run: &str, switch: ProfileSwitch) -> Result<()> {
        synced!(self, TaskHeadMut::push_switch(self, run, switch))
    }

    fn push_wait(&mut self, run: &str, wait: WaitRecord) -> Result<()> {
        synced!(self, TaskHeadMut::push_wait(self, run, wait))
    }

    fn push_decision(&mut self, run: &str, decision: DecisionRecord) -> Result<()> {
        synced!(self, TaskHeadMut::push_decision(self, run, decision))
    }

    fn edit_wait(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut WaitRecord) -> Result<()>,
    ) -> Result<()> {
        synced!(self, TaskHeadMut::edit_wait(self, run, id, edit))
    }

    fn edit_decision(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut DecisionRecord) -> Result<()>,
    ) -> Result<()> {
        synced!(self, TaskHeadMut::edit_decision(self, run, id, edit))
    }

    fn mark_effect_applied(&mut self, recipient: &Recipient, effect: u64) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::mark_effect_applied(self, recipient, effect)
        )
    }

    fn set_observe_cursor(&mut self, recipient: &Recipient, cursor: SessionCursor) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::set_observe_cursor(self, recipient, cursor)
        )
    }

    fn append_usage_entries(
        &mut self,
        run: &str,
        child: Option<&str>,
        entries: Vec<UsageEntry>,
    ) -> Result<()> {
        synced!(
            self,
            TaskHeadMut::append_usage_entries(self, run, child, entries)
        )
    }

    fn set_usage(&mut self, run: &str, child: Option<&str>, usage: Usage) -> Result<()> {
        synced!(self, TaskHeadMut::set_usage(self, run, child, usage))
    }
}
