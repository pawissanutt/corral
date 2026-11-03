//! Review mutations of a head's current plan and aggregate attempt (plan T-253 section 4,
//! "Child mutations"): pushes demote the current review, keyed edits rewrite one review, and
//! the bulk forms iterate only their pending pages.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::PlanReviewHead;
use super::assemble::{TaskRows, inbox_value};
use super::codec::{Rows, child_key, plan_review_rows, seq};
use super::head::last_ordinal;
use super::head_mut::{Demoted, TaskHeadMut, add_panes};
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::container_facts::{PaneRef, herdr_panes};
use crate::work::Counter;
use crate::{AggregateReview, ContainerFacts, PlanReview};

type RowMap = BTreeMap<(TaskPart, String), HistoryRow>;

impl TaskHeadMut {
    pub(super) fn current_plan_seq(&self, plan: &str) -> Result<usize> {
        match (&self.head.last_plan, self.head.seqs.plan) {
            (Some(current), Some(seq)) if current.id == plan => Ok(seq),
            _ => bail!(
                "plan {plan} is not the current plan of {}",
                self.head.task.id
            ),
        }
    }

    fn adjust_plan_review(
        facts: &mut ContainerFacts,
        review: &PlanReview,
        owner: &str,
        sign: i64,
    ) -> Vec<PaneRef> {
        facts.changes = facts.changes.saturating_add_signed(
            sign * i64::from(review.verdict == Some(crate::Verdict::Changes)),
        );
        facts.pending_children = facts
            .pending_children
            .saturating_add_signed(sign * i64::from(review.verdict.is_none() && !review.cancelled));
        let panes = herdr_panes(owner, "plan_review", review.herdr.as_ref());
        add_panes(&mut facts.pane_refs, &panes, sign);
        panes
    }

    pub fn push_plan_review(&mut self, plan: &str, review: PlanReview) -> Result<()> {
        self.adjust_projections()?;
        let p = self.current_plan_seq(plan)?;
        let exists = self.rows_now(
            &RowQuery::under(TaskPart::PlanReview, format!("{}/", seq(p)))
                .with_index(format!("id:{}", review.id)),
            Counter::NestedRowDecodes,
        )?;
        ensure!(
            exists.is_empty(),
            "plan review {} already exists on {plan}",
            review.id
        );
        let current = self.head.last_plan.as_mut().context("no current plan")?;
        let n = usize::try_from(current.facts.children)? + 1;
        if let (Some(old), Some(r)) = (current.current_review.take(), self.head.seqs.plan_review) {
            self.demoted.push(Demoted::PlanReview(p, r, old));
        }
        let mut rows = Rows::new();
        plan_review_rows(
            &mut rows,
            &seq(p),
            n,
            &review,
            serde_json::to_value(&review)?,
            true,
        )?;
        let own = rows
            .0
            .remove(&(TaskPart::PlanReview, child_key(p, n)))
            .context("the plan review has no row")?;
        let current = self.head.last_plan.as_mut().context("no current plan")?;
        current.facts.children += 1;
        current.facts.children_rev += 1;
        let owner = format!("plan_review:{}", child_key(p, n));
        let panes = Self::adjust_plan_review(&mut current.facts, &review, &owner, 1);
        current.current_review = Some(serde_json::from_value(own.doc)?);
        self.head.seqs.plan_review = Some(n);
        for row in rows.0.into_values() {
            self.put(row);
        }
        for pane in &panes {
            self.put(super::head_mut::pane_row(pane));
        }
        *self.seen = self.head.clone();
        Ok(())
    }

    fn plan_review_parts(&self, row: &HistoryRow) -> Result<(RowMap, PlanReview)> {
        let owner = format!("plan_review:{}", row.key);
        let mut rows = RowMap::new();
        rows.insert((row.part, row.key.clone()), row.clone());
        for part in [TaskPart::InboxEffect, TaskPart::ObserveCursor] {
            for row in self.rows_now(
                &RowQuery::under(part, format!("{owner}/")),
                Counter::NestedRowDecodes,
            )? {
                rows.insert((row.part, row.key.clone()), row);
            }
        }
        let view = TaskRows::new(rows.values());
        let mut doc = row.doc.as_object().cloned().unwrap_or_default();
        inbox_value(&view, &owner, &mut doc)?;
        Ok((rows, serde_json::from_value(Value::Object(doc))?))
    }

    pub fn edit_plan_review(
        &mut self,
        plan: &str,
        id: &str,
        edit: impl FnOnce(&mut PlanReview) -> Result<()>,
    ) -> Result<()> {
        self.adjust_projections()?;
        let p = self.current_plan_seq(plan)?;
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
        let (old_rows, before) = self.plan_review_parts(&row)?;
        let mut after = before.clone();
        edit(&mut after)?;
        ensure!(
            after.id == before.id,
            "a plan review edit may not change its id"
        );
        let mut rows = Rows::new();
        plan_review_rows(
            &mut rows,
            &seq(p),
            n,
            &after,
            serde_json::to_value(&after)?,
            row.current,
        )?;
        let changed = rows.0 != old_rows;
        let new_doc = rows.0[&(TaskPart::PlanReview, child_key(p, n))].doc.clone();
        self.write_diff(&old_rows, &rows.0)?;
        let owner = format!("plan_review:{}", child_key(p, n));
        let plan_head = self.head.last_plan.as_mut().context("no current plan")?;
        let before_panes = Self::adjust_plan_review(&mut plan_head.facts, &before, &owner, -1);
        let after_panes = Self::adjust_plan_review(&mut plan_head.facts, &after, &owner, 1);
        if changed {
            plan_head.facts.children_rev += 1;
        }
        if row.current {
            plan_head.current_review = Some(serde_json::from_value(new_doc)?);
        }
        self.write_panes(&before_panes, &after_panes);
        *self.seen = self.head.clone();
        Ok(())
    }

    /// Removes the current review of `plan` and makes the previous one current again.
    pub fn pop_current_plan_review(&mut self, plan: &str) -> Result<()> {
        self.adjust_projections()?;
        let p = self.current_plan_seq(plan)?;
        let r = self
            .head
            .seqs
            .plan_review
            .context("the plan has no current review")?;
        let row = self
            .rows_now(
                &RowQuery::key(TaskPart::PlanReview, child_key(p, r)),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .context("the current plan review row is missing")?;
        let (old_rows, review) = self.plan_review_parts(&row)?;
        for (part, key) in old_rows.keys() {
            self.delete(*part, key.clone());
        }
        let owner = format!("plan_review:{}", child_key(p, r));
        let previous = if r > 1 {
            let mut prev = self
                .rows_now(
                    &RowQuery::key(TaskPart::PlanReview, child_key(p, r - 1)),
                    Counter::NestedRowDecodes,
                )?
                .into_iter()
                .next()
                .context("the previous plan review row is missing")?;
            prev.current = true;
            let head: PlanReviewHead = serde_json::from_value(prev.doc.clone())?;
            self.put(prev);
            Some(head)
        } else {
            None
        };
        let plan_head = self.head.last_plan.as_mut().context("no current plan")?;
        plan_head.facts.children = plan_head.facts.children.saturating_sub(1);
        plan_head.facts.children_rev += 1;
        let panes = Self::adjust_plan_review(&mut plan_head.facts, &review, &owner, -1);
        plan_head.current_review = previous;
        self.head.seqs.plan_review = (r > 1).then(|| r - 1);
        for pane in panes {
            self.delete(TaskPart::PaneRef, pane.key());
        }
        *self.seen = self.head.clone();
        Ok(())
    }

    /// Ends every pending review of `plan` with an escalation carrying `reason`.
    pub fn terminalize_pending_plan_reviews(&mut self, plan: &str, reason: &str) -> Result<()> {
        let p = self.current_plan_seq(plan)?;
        let pending: Vec<String> = self
            .rows_now(
                &RowQuery::under(TaskPart::PlanReview, format!("{}/", seq(p))).open(),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .map(|row| Ok(serde_json::from_value::<PlanReviewHead>(row.doc)?.id))
            .collect::<Result<_>>()?;
        for id in pending {
            self.edit_plan_review_head(plan, &id, |review| {
                review.verdict = Some(crate::Verdict::Escalate);
                review.notes = Some(reason.into());
                review.pending_reply = None;
                review.at = crate::clock::now();
                Ok(())
            })?;
        }
        Ok(())
    }

    fn current_attempt_seq(&self, attempt: &str) -> Result<usize> {
        match (&self.head.last_aggregate_attempt, self.head.seqs.attempt) {
            (Some(current), Some(seq)) if current.id == attempt => Ok(seq),
            _ => bail!(
                "aggregate attempt {attempt} is not current on {}",
                self.head.task.id
            ),
        }
    }

    fn aggregate_row(
        a: usize,
        n: usize,
        review: &AggregateReview,
        current: bool,
    ) -> Result<HistoryRow> {
        super::head_mut::aggregate_review_row(a, n, review, current)
    }

    pub fn push_aggregate_review(&mut self, attempt: &str, review: AggregateReview) -> Result<()> {
        self.adjust_projections()?;
        let a = self.current_attempt_seq(attempt)?;
        let exists = self.rows_now(
            &RowQuery::under(TaskPart::AggregateReview, format!("{}/", seq(a)))
                .with_index(format!("id:{}", review.id)),
            Counter::NestedRowDecodes,
        )?;
        ensure!(
            exists.is_empty(),
            "aggregate review {} already exists on {attempt}",
            review.id
        );
        let current = self
            .head
            .last_aggregate_attempt
            .as_mut()
            .context("no current attempt")?;
        let n = usize::try_from(current.facts.children)? + 1;
        if let (Some(old), Some(r)) = (current.current_review.take(), self.head.seqs.attempt_review)
        {
            self.demoted.push(Demoted::AggregateReview(a, r, old));
        }
        let current = self
            .head
            .last_aggregate_attempt
            .as_mut()
            .context("no current attempt")?;
        current.facts.children += 1;
        current.facts.children_rev += 1;
        current.facts.pending_children += u64::from(review.pending());
        let panes = herdr_panes(
            &format!("aggregate_review:{}", child_key(a, n)),
            "aggregate_review",
            review.herdr.as_ref(),
        );
        add_panes(&mut current.facts.pane_refs, &panes, 1);
        current.current_review = Some(review);
        self.head.seqs.attempt_review = Some(n);
        for pane in &panes {
            self.put(super::head_mut::pane_row(pane));
        }
        *self.seen = self.head.clone();
        Ok(())
    }

    pub fn edit_aggregate_review(
        &mut self,
        attempt: &str,
        id: &str,
        edit: impl FnOnce(&mut AggregateReview) -> Result<()>,
    ) -> Result<()> {
        self.adjust_projections()?;
        let a = self.current_attempt_seq(attempt)?;
        let row = self
            .rows_now(
                &RowQuery::under(TaskPart::AggregateReview, format!("{}/", seq(a)))
                    .with_index(format!("id:{id}")),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .with_context(|| format!("aggregate review {id} not found on {attempt}"))?;
        let n = last_ordinal(&row.key)?;
        let before: AggregateReview = serde_json::from_value(row.doc.clone())?;
        let mut after = before.clone();
        edit(&mut after)?;
        ensure!(
            after.id == before.id,
            "an aggregate review edit may not change its id"
        );
        let new = Self::aggregate_row(a, n, &after, row.current)?;
        let owner = format!("aggregate_review:{}", child_key(a, n));
        let (before_panes, after_panes) = (
            herdr_panes(&owner, "aggregate_review", before.herdr.as_ref()),
            herdr_panes(&owner, "aggregate_review", after.herdr.as_ref()),
        );
        let changed = new.doc != row.doc;
        let attempt_head = self
            .head
            .last_aggregate_attempt
            .as_mut()
            .context("no current attempt")?;
        attempt_head.facts.pending_children = attempt_head
            .facts
            .pending_children
            .saturating_add_signed(i64::from(after.pending()) - i64::from(before.pending()));
        add_panes(&mut attempt_head.facts.pane_refs, &before_panes, -1);
        add_panes(&mut attempt_head.facts.pane_refs, &after_panes, 1);
        if changed {
            attempt_head.facts.children_rev += 1;
        }
        if row.current {
            attempt_head.current_review = Some(after);
        } else {
            self.history_edit = true;
            self.put(new);
        }
        self.write_panes(&before_panes, &after_panes);
        *self.seen = self.head.clone();
        Ok(())
    }

    /// Abandons every pending review of `attempt`, superseded by review `by`.
    pub fn supersede_pending_aggregate_reviews(&mut self, attempt: &str, by: &str) -> Result<()> {
        let a = self.current_attempt_seq(attempt)?;
        let pending: Vec<String> = self
            .rows_now(
                &RowQuery::under(TaskPart::AggregateReview, format!("{}/", seq(a))).open(),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .map(|row| Ok(serde_json::from_value::<AggregateReview>(row.doc)?.id))
            .collect::<Result<_>>()?;
        for id in pending {
            self.edit_aggregate_review(attempt, &id, |review| {
                review.abandoned = Some(format!("superseded by {by}"));
                Ok(())
            })?;
        }
        Ok(())
    }
}
