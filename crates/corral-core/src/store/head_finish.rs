//! The commit boundary of a head mutation: identities checked, revision counters advanced once
//! per write, and the queued, demoted and current rows turned into the rows one `UpdateTask`
//! writes.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::RunHead;
use super::codec::SELF;
use super::head::TaskHead;
use super::head_mut::{
    Demoted, TaskHeadMut, aggregate_review_row, attempt_row, plan_review_row, plan_row,
    plan_run_row, run_row,
};
use crate::backend::{HistoryRow, RowWrite, TaskPart};

impl TaskHeadMut {
    /// The rows this mutation writes: queued child rows, demoted containers, and every current
    /// container whose stored document changes.
    pub(crate) fn finish(mut self) -> Result<(TaskHead, Vec<RowWrite>, bool)> {
        self.adjust_projections()?;
        if self.usage_dirty {
            self.recompute_usage()?;
        }
        self.one_revision_per_write();
        let mut writes: BTreeMap<(TaskPart, String), Option<HistoryRow>> =
            std::mem::take(&mut self.rows);
        // A queued row of a demoted container is an edit made after its demotion, so it wins.
        for row in self.demoted_rows()? {
            writes
                .entry((row.part, row.key.clone()))
                .or_insert(Some(row));
        }
        let mut put = |row: HistoryRow| {
            writes.insert((row.part, row.key.clone()), Some(row));
        };
        let changed = |new: &HistoryRow, old: Option<HistoryRow>| old.as_ref() != Some(new);
        let (head, base) = (&self.head, &self.base);
        if let (Some(run), Some(n)) = (&head.current_run, head.seqs.run) {
            let row = run_row(n, SELF, run, true)?;
            let old = base
                .current_run
                .as_ref()
                .filter(|_| base.seqs.run == Some(n))
                .map(|old| run_row(n, SELF, old, true))
                .transpose()?;
            if changed(&row, old) {
                put(row);
            }
            if let (Some(child), Some(c)) = (run.current_child.as_deref(), head.seqs.run_child) {
                let row = run_row(n, c, child, true)?;
                let old = base
                    .current_run
                    .as_ref()
                    .and_then(|r| r.current_child.as_deref())
                    .filter(|_| base.seqs.run == Some(n) && base.seqs.run_child == Some(c))
                    .map(|old| run_row(n, c, old, true))
                    .transpose()?;
                if changed(&row, old) {
                    put(row);
                }
            }
        }
        if let (Some(run), Some(n)) = (&head.last_plan_run, head.seqs.plan_run) {
            let row = plan_run_row(n, run, true)?;
            let old = base
                .last_plan_run
                .as_ref()
                .filter(|_| base.seqs.plan_run == Some(n))
                .map(|old| plan_run_row(n, old, true))
                .transpose()?;
            if changed(&row, old) {
                put(row);
            }
        }
        if let (Some(plan), Some(n)) = (&head.last_plan, head.seqs.plan) {
            let row = plan_row(n, plan, true)?;
            let same = base.seqs.plan == Some(n);
            let old = base
                .last_plan
                .as_ref()
                .filter(|_| same)
                .map(|old| plan_row(n, old, true))
                .transpose()?;
            if changed(&row, old) {
                put(row);
            }
            if let (Some(review), Some(r)) = (&plan.current_review, head.seqs.plan_review) {
                let row = plan_review_row(n, r, review, true)?;
                let old = base
                    .last_plan
                    .as_ref()
                    .and_then(|p| p.current_review.as_ref())
                    .filter(|_| same && base.seqs.plan_review == Some(r))
                    .map(|old| plan_review_row(n, r, old, true))
                    .transpose()?;
                if changed(&row, old) {
                    put(row);
                }
            }
        }
        if let (Some(attempt), Some(n)) = (&head.last_aggregate_attempt, head.seqs.attempt) {
            let row = attempt_row(n, attempt, true)?;
            let same = base.seqs.attempt == Some(n);
            let old = base
                .last_aggregate_attempt
                .as_ref()
                .filter(|_| same)
                .map(|old| attempt_row(n, old, true))
                .transpose()?;
            if changed(&row, old) {
                put(row);
            }
            if let (Some(review), Some(r)) = (&attempt.current_review, head.seqs.attempt_review) {
                let row = aggregate_review_row(n, r, review, true)?;
                let old = base
                    .last_aggregate_attempt
                    .as_ref()
                    .and_then(|a| a.current_review.as_ref())
                    .filter(|_| same && base.seqs.attempt_review == Some(r))
                    .map(|old| aggregate_review_row(n, r, old, true))
                    .transpose()?;
                if changed(&row, old) {
                    put(row);
                }
            }
        }
        let mut rows = Vec::with_capacity(writes.len());
        for ((part, key), row) in writes {
            match row {
                Some(row) => rows.push(RowWrite::Put(row)),
                // A row queued and removed within this mutation was never stored.
                None if self
                    .head
                    .query(
                        &crate::backend::RowQuery::key(part, key.clone()),
                        1,
                        crate::work::Counter::NestedRowDecodes,
                    )?
                    .is_empty() => {}
                None => rows.push(RowWrite::Delete { part, key }),
            }
        }
        Ok((self.head, rows, self.history_edit))
    }

    /// A container's revision counters advance at most once per write, as the full write path
    /// advances them: the helpers count each child change, the write counts that any happened.
    fn one_revision_per_write(&mut self) {
        fn clamp(rev: &mut u64, base: u64) {
            if *rev > base {
                *rev = base + 1;
            }
        }
        let (base, head) = (&self.base, &mut self.head);
        if let (Some(run), Some(old)) = (head.current_run.as_mut(), base.current_run.as_ref())
            && head.seqs.run == base.seqs.run
        {
            clamp(&mut run.facts.children_rev, old.facts.children_rev);
            clamp(&mut run.facts.obligations_rev, old.facts.obligations_rev);
            if let (Some(child), Some(old_child)) = (
                run.current_child.as_deref_mut(),
                old.current_child.as_deref(),
            ) && head.seqs.run_child == base.seqs.run_child
            {
                clamp(
                    &mut child.facts.obligations_rev,
                    old_child.facts.obligations_rev,
                );
            }
        }
        if let (Some(plan), Some(old)) = (head.last_plan.as_mut(), base.last_plan.as_ref())
            && head.seqs.plan == base.seqs.plan
        {
            clamp(&mut plan.facts.children_rev, old.facts.children_rev);
        }
        if let (Some(attempt), Some(old)) = (
            head.last_aggregate_attempt.as_mut(),
            base.last_aggregate_attempt.as_ref(),
        ) && head.seqs.attempt == base.seqs.attempt
        {
            clamp(&mut attempt.facts.children_rev, old.facts.children_rev);
        }
    }

    fn demoted_rows(&self) -> Result<Vec<HistoryRow>> {
        self.demoted
            .iter()
            .map(|demoted| match demoted {
                Demoted::Run(n, run) => run_row(*n, SELF, run, false),
                Demoted::RunChild(n, c, child) => run_row(*n, *c, child, false),
                Demoted::PlanRun(n, run) => plan_run_row(*n, run, false),
                Demoted::Plan(n, plan) => plan_row(*n, plan, false),
                Demoted::PlanReview(p, n, review) => plan_review_row(*p, *n, review, false),
                Demoted::Attempt(n, attempt) => attempt_row(*n, attempt, false),
                Demoted::AggregateReview(a, n, review) => {
                    aggregate_review_row(*a, *n, review, false)
                }
            })
            .collect()
    }

    /// The rows this mutation holds outside its queue, as `finish` will store them: demoted
    /// containers (unless an edit queued a newer row) and every current container and child.
    pub(crate) fn pending_rows(&self) -> Result<Vec<HistoryRow>> {
        let mut rows: Vec<HistoryRow> = self
            .demoted_rows()?
            .into_iter()
            .filter(|row| !self.rows.contains_key(&(row.part, row.key.clone())))
            .collect();
        let head = &self.head;
        if let (Some(run), Some(n)) = (&head.current_run, head.seqs.run) {
            rows.push(run_row(n, SELF, run, true)?);
            if let (Some(child), Some(c)) = (run.current_child.as_deref(), head.seqs.run_child) {
                rows.push(run_row(n, c, child, true)?);
            }
        }
        if let (Some(run), Some(n)) = (&head.last_plan_run, head.seqs.plan_run) {
            rows.push(plan_run_row(n, run, true)?);
        }
        if let (Some(plan), Some(n)) = (&head.last_plan, head.seqs.plan) {
            rows.push(plan_row(n, plan, true)?);
            if let (Some(review), Some(r)) = (&plan.current_review, head.seqs.plan_review) {
                rows.push(plan_review_row(n, r, review, true)?);
            }
        }
        if let (Some(attempt), Some(n)) = (&head.last_aggregate_attempt, head.seqs.attempt) {
            rows.push(attempt_row(n, attempt, true)?);
            if let (Some(review), Some(r)) = (&attempt.current_review, head.seqs.attempt_review) {
                rows.push(aggregate_review_row(n, r, review, true)?);
            }
        }
        Ok(rows)
    }

    /// Refuses a direct projection edit that renames, restarts, adds or removes a container or
    /// child in place: only the keyed helpers change which container a position holds.
    pub(super) fn ensure_stable_identities(base: &TaskHead, head: &TaskHead) -> Result<()> {
        fn same<T, K: PartialEq>(a: Option<&T>, b: Option<&T>, key: impl Fn(&T) -> K) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => key(a) == key(b),
                (None, None) => true,
                _ => false,
            }
        }
        let (old, new) = (&base.seqs, &head.seqs);
        let run = |r: &RunHead| (r.id.clone(), r.started);
        let checks = [
            (
                "the current run",
                old.run != new.run
                    || same(base.current_run.as_ref(), head.current_run.as_ref(), run),
            ),
            (
                "the current review child",
                old.run != new.run
                    || old.run_child != new.run_child
                    || same(
                        base.current_run
                            .as_ref()
                            .and_then(|r| r.current_child.as_deref()),
                        head.current_run
                            .as_ref()
                            .and_then(|r| r.current_child.as_deref()),
                        run,
                    ),
            ),
            (
                "the planning run",
                old.plan_run != new.plan_run
                    || same(
                        base.last_plan_run.as_ref(),
                        head.last_plan_run.as_ref(),
                        |r| (r.id.clone(), r.started),
                    ),
            ),
            (
                "the current plan",
                old.plan != new.plan
                    || same(base.last_plan.as_ref(), head.last_plan.as_ref(), |p| {
                        p.id.clone()
                    }),
            ),
            (
                "the current plan review",
                old.plan != new.plan
                    || old.plan_review != new.plan_review
                    || same(
                        base.last_plan
                            .as_ref()
                            .and_then(|p| p.current_review.as_ref()),
                        head.last_plan
                            .as_ref()
                            .and_then(|p| p.current_review.as_ref()),
                        |r| r.id.clone(),
                    ),
            ),
            (
                "the current aggregate attempt",
                old.attempt != new.attempt
                    || same(
                        base.last_aggregate_attempt.as_ref(),
                        head.last_aggregate_attempt.as_ref(),
                        |a| (a.id.clone(), a.started),
                    ),
            ),
            (
                "the current aggregate review",
                old.attempt != new.attempt
                    || old.attempt_review != new.attempt_review
                    || same(
                        base.last_aggregate_attempt
                            .as_ref()
                            .and_then(|a| a.current_review.as_ref()),
                        head.last_aggregate_attempt
                            .as_ref()
                            .and_then(|a| a.current_review.as_ref()),
                        |r| r.id.clone(),
                    ),
            ),
        ];
        for (what, stable) in checks {
            ensure!(
                stable,
                "{what} may not change its id, start or presence in place"
            );
        }
        Ok(())
    }
}
