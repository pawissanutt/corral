//! Scalar edits of one review child or plan review as its projection (plan T-253 section 4,
//! "The rule one level down"): the edited child's own row is read and written, its nested
//! rows are neither read nor rewritten, and the container's facts change by the delta.

use anyhow::{Context, Result, ensure};

use super::codec::{child_key, seq};
use super::head::{last_ordinal, projection};
use super::head_mut::{TaskHeadMut, add_count, add_panes, plan_review_row, run_own_panes, run_row};
use super::task_mut_task::check_run_edit;
use super::{PlanReviewHead, RunHead};
use crate::backend::{RowQuery, TaskPart};
use crate::container_facts::herdr_panes;
use crate::work::Counter;

pub(crate) fn check_plan_review_edit(
    before: &PlanReviewHead,
    after: &PlanReviewHead,
) -> Result<()> {
    ensure!(
        before.id == after.id,
        "a plan review edit may not change its id"
    );
    ensure!(
        before.inbox == after.inbox,
        "a plan review edit may not change its inbox"
    );
    Ok(())
}

fn plan_review_facts(
    facts: &mut crate::ContainerFacts,
    review: &PlanReviewHead,
    owner: &str,
    sign: i64,
) -> Vec<crate::PaneRef> {
    facts.changes = facts
        .changes
        .saturating_add_signed(sign * i64::from(review.verdict == Some(crate::Verdict::Changes)));
    facts.pending_children = facts
        .pending_children
        .saturating_add_signed(sign * i64::from(review.verdict.is_none()));
    let panes = herdr_panes(owner, "plan_review", review.herdr.as_ref());
    add_panes(&mut facts.pane_refs, &panes, sign);
    panes
}

impl TaskHeadMut {
    /// Edits review child `id` of the current run `run` as its projection.
    ///
    /// # Errors
    /// Refuses a run that is not current, a missing child, or an edit of the child's id,
    /// start, end, usage, inbox, agent records or facts.
    pub fn edit_review_child_head(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut RunHead) -> Result<()>,
    ) -> Result<()> {
        self.adjust_projections()?;
        let s = self.current_run_seq(run)?;
        if let Some(current) = self
            .head
            .current_run
            .as_mut()
            .and_then(|r| r.current_child.as_deref_mut())
            .filter(|c| c.id == id)
        {
            let mut after = current.clone();
            edit(&mut after)?;
            check_run_edit(current, &after)?;
            *current = after;
            return Ok(());
        }
        let row = self
            .rows_now(
                &RowQuery::under(TaskPart::ReviewRun, format!("{}/", seq(s)))
                    .with_index(format!("id:{id}")),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .with_context(|| format!("review child {id} not found on {run}"))?;
        let n = last_ordinal(&row.key)?;
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
        let (old_panes, new_panes) = (run_own_panes(s, n, &before), run_own_panes(s, n, &after));
        add_panes(&mut after.facts.pane_refs, &old_panes, -1);
        add_panes(&mut after.facts.pane_refs, &new_panes, 1);
        let new_row = run_row(s, n, &after, row.current)?;
        let changed = new_row.doc != row.doc;
        let parent = self.head.current_run.as_mut().context("no current run")?;
        Self::adjust_child(&mut parent.facts, &before, -1);
        Self::adjust_child(&mut parent.facts, &after, 1);
        add_panes(&mut parent.facts.pane_refs, &old_panes, -1);
        add_panes(&mut parent.facts.pane_refs, &new_panes, 1);
        if changed {
            parent.facts.children_rev += 1;
        }
        self.write_panes(&old_panes, &new_panes);
        self.put(new_row);
        self.seen = self.head.clone();
        Ok(())
    }

    /// Edits review `id` of the current plan `plan` as its projection.
    ///
    /// # Errors
    /// Refuses a plan that is not current, a missing review, or an edit of its id or inbox.
    pub fn edit_plan_review_head(
        &mut self,
        plan: &str,
        id: &str,
        edit: impl FnOnce(&mut PlanReviewHead) -> Result<()>,
    ) -> Result<()> {
        self.adjust_projections()?;
        let p = self.current_plan_seq(plan)?;
        if let Some(current) = self
            .head
            .last_plan
            .as_mut()
            .and_then(|p| p.current_review.as_mut())
            .filter(|r| r.id == id)
        {
            let mut after = current.clone();
            edit(&mut after)?;
            check_plan_review_edit(current, &after)?;
            *current = after;
            return Ok(());
        }
        let row = self
            .rows_now(
                &RowQuery::under(TaskPart::PlanReview, format!("{}/", seq(p)))
                    .with_index(format!("id:{id}")),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .with_context(|| format!("plan review {id} not found on {plan}"))?;
        let n = last_ordinal(&row.key)?;
        let before: PlanReviewHead = serde_json::from_value(row.doc.clone())?;
        let mut after = before.clone();
        edit(&mut after)?;
        check_plan_review_edit(&before, &after)?;
        let new_row = plan_review_row(p, n, &after, row.current)?;
        let changed = new_row.doc != row.doc;
        let owner = format!("plan_review:{}", child_key(p, n));
        let plan_head = self.head.last_plan.as_mut().context("no current plan")?;
        let old_panes = plan_review_facts(&mut plan_head.facts, &before, &owner, -1);
        let new_panes = plan_review_facts(&mut plan_head.facts, &after, &owner, 1);
        if changed {
            plan_head.facts.children_rev += 1;
        }
        self.write_panes(&old_panes, &new_panes);
        self.put(new_row);
        self.seen = self.head.clone();
        Ok(())
    }
}
