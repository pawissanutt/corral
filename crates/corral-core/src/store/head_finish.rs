//! The commit boundary of a head mutation: identities checked, revision counters advanced once
//! per write, and the queued, demoted and current rows turned into the rows one `UpdateTask`
//! writes.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use super::RunHead;
use super::codec::{FACTS, SELF, child_key, seq};
use super::head::TaskHead;
use super::head_mut::{
    Demoted, TaskHeadMut, aggregate_review_row, attempt_row, pane_row, plan_review_row, plan_row,
    plan_run_row, run_row,
};
use crate::backend::{HistoryRow, RowWrite, TaskPart};

impl TaskHeadMut {
    /// The rows this mutation writes: queued child rows, demoted containers, and every current
    /// container whose stored document changes.
    pub(crate) fn finish(mut self) -> Result<(TaskHead, Vec<RowWrite>, bool)> {
        self.sync()?;
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
        let before = super::codec::launch_pane(self.base.task.launch.as_ref());
        let after = super::codec::launch_pane(self.head.task.launch.as_ref());
        if before != after {
            match after {
                Some(pane) => {
                    let row = pane_row(&pane);
                    writes.insert((row.part, row.key.clone()), Some(row));
                }
                None => {
                    if let Some(pane) = before {
                        writes.insert((TaskPart::PaneRef, pane.key()), None);
                    }
                }
            }
        }
        let (rows, history_edit) = self.settle(writes)?;
        Ok((self.head, rows, history_edit))
    }

    /// Recomputes the summary's operating-contract revision when a contract input or the scope
    /// history changed; only then is the scope history read.
    /// Brings every derived fact up to date with the edits so far: projection facts and pane
    /// rows, the current run's usage totals and the contract revision.
    pub(crate) fn sync(&mut self) -> Result<()> {
        self.adjust_projections()?;
        if self.usage_dirty {
            self.recompute_usage()?;
            self.usage_dirty = false;
        }
        let settled = (self.head.task.clone(), self.head.summary.scope_revisions);
        if self.contract_settled.as_ref() != Some(&settled) {
            self.settle_contract_revision()?;
            self.contract_settled = Some(settled);
        }
        Ok(())
    }

    fn settle_contract_revision(&mut self) -> Result<()> {
        use crate::operating::operating_contract_revision;
        let mut after = super::head_api::shell_task(&self.head.task)?;
        let before = super::head_api::shell_task(&self.base.task)?;
        if !self.scope_history_changed
            && operating_contract_revision(&before)? == operating_contract_revision(&after)?
        {
            self.head.summary.operating_contract_revision =
                self.base.summary.operating_contract_revision.clone();
            return Ok(());
        }
        if self.head.summary.scope_revisions > 0 {
            let rows = self.rows_now(
                &crate::backend::RowQuery::part(TaskPart::ScopeRevision),
                crate::work::Counter::HistoryRowDecodes,
            )?;
            after.approved_scope_history = rows
                .into_iter()
                .map(|row| serde_json::from_value(row.doc))
                .collect::<Result<_, _>>()?;
        }
        self.head.summary.operating_contract_revision = operating_contract_revision(&after)?;
        Ok(())
    }

    /// Turns the final write set into the operation, judged against the stored documents: a
    /// container's revision counters advance once from their stored value when a stored child
    /// document they guard changed, rows equal to what is stored are dropped, and only a write
    /// that touches a stored history row needs the full form.
    fn settle(
        &mut self,
        mut writes: BTreeMap<Key, Option<HistoryRow>>,
    ) -> Result<(Vec<RowWrite>, bool)> {
        let base_rows: BTreeMap<Key, HistoryRow> = projection_rows(&self.base)?
            .into_iter()
            .map(|row| ((row.part, row.key.clone()), row))
            .collect();
        let mut stored: BTreeMap<Key, Option<HistoryRow>> = BTreeMap::new();
        for (key, write) in &writes {
            // A checkpoint payload is keyed by its content digest: a stored one is identical to
            // the one put, so a put never reads it to compare. A delete still does.
            let row = if key.0 == TaskPart::CheckpointPayload && write.is_some() {
                None
            } else {
                self.stored_row(&base_rows, key)?
            };
            stored.insert(key.clone(), row);
        }
        // Settle leaves before their parents: a review child's obligations_rev is part of
        // the document guarded by its run's children_rev, not an independent witness.
        for field in ["obligations_rev", "children_rev"] {
            let mut advanced = std::collections::BTreeSet::new();
            for (key, row) in &writes {
                let changed = match (row, &stored[key]) {
                    (Some(new), Some(old)) => new.doc != old.doc,
                    (None, None) => false,
                    _ => true,
                };
                if changed
                    && let Some((container, guarded_field)) = guarded_by(key.0, &key.1)
                    && guarded_field == field
                {
                    advanced.insert(container);
                }
            }
            let mut containers: std::collections::BTreeSet<Key> = writes
                .iter()
                .filter(|(key, row)| row.is_some() && revision_fields(key.0).contains(&field))
                .map(|(key, _)| key.clone())
                .collect();
            containers.extend(advanced.iter().cloned());
            for key in containers {
                if !stored.contains_key(&key) {
                    stored.insert(key.clone(), self.stored_row(&base_rows, &key)?);
                }
                let original = stored[&key].clone();
                let mut row = match writes.get(&key) {
                    Some(Some(row)) => row.clone(),
                    Some(None) => continue,
                    None => match &original {
                        Some(row) => row.clone(),
                        None => continue,
                    },
                };
                let base = original.as_ref().map_or(0, |row| facts_rev(row, field));
                set_rev(&mut row, field, base + u64::from(advanced.contains(&key)));
                writes.insert(key, Some(row));
            }
        }
        self.sync_projection_revisions(&writes);
        for (key, row, old) in self.finding_writes(&writes, &stored)? {
            stored.insert(key.clone(), old);
            writes.insert(key, row);
        }
        // The cleanup ledger gains each pane a written container names and keeps every row it
        // already has, released or not.
        // Only panes a written row names anew are looked up: its stored version's panes are
        // already in the ledger.
        let mut derived: Vec<HistoryRow> = Vec::new();
        for (key, row) in &writes {
            let Some(row) = row else { continue };
            let known: std::collections::BTreeSet<String> = stored
                .get(key)
                .cloned()
                .flatten()
                .map(|old| super::cleanup_panes::ledger_of(&old))
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.key)
                .collect();
            derived.extend(
                super::cleanup_panes::ledger_of(row)
                    .into_iter()
                    .filter(|r| !known.contains(&r.key)),
            );
        }
        for row in derived {
            let key = (row.part, row.key.clone());
            if writes.contains_key(&key) {
                continue;
            }
            let old = self.stored_row(&base_rows, &key)?;
            if old.is_none() {
                stored.insert(key.clone(), None);
                writes.insert(key, Some(row));
            }
        }
        let mut rows = Vec::with_capacity(writes.len());
        let mut history_edit = false;
        for (key, row) in writes {
            let original = stored.remove(&key).flatten();
            match (row, original) {
                (Some(row), Some(old)) if row == old => {}
                (None, None) => {}
                (row, old) => {
                    history_edit |= key.0 != TaskPart::CleanupPane
                        && old.as_ref().is_some_and(|old| !old.current);
                    rows.push(match row {
                        Some(row) => RowWrite::Put(row),
                        None => RowWrite::Delete {
                            part: key.0,
                            key: key.1,
                        },
                    });
                }
            }
        }
        Ok((rows, history_edit))
    }

    /// The stored row at `key`: one this mutation already read, a current container's row as
    /// read into the base head, or one keyed read.
    fn stored_row(
        &self,
        base_rows: &BTreeMap<Key, HistoryRow>,
        key: &Key,
    ) -> Result<Option<HistoryRow>> {
        if let Some(row) = self.originals.borrow().get(key) {
            return Ok(Some(row.clone()));
        }
        if let Some(row) = base_rows.get(key) {
            return Ok(Some(row.clone()));
        }
        Ok(self
            .head
            .query(
                &crate::backend::RowQuery::key(key.0, key.1.clone()),
                1,
                crate::work::Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next())
    }

    /// Copies the final revision counters into the current projections the returned head holds.
    fn sync_projection_revisions(&mut self, writes: &BTreeMap<Key, Option<HistoryRow>>) {
        let rev = |part: TaskPart, key: String, field: &str| {
            writes
                .get(&(part, key))
                .and_then(Option::as_ref)
                .map(|row| facts_rev(row, field))
        };
        let head = &mut self.head;
        if let (Some(run), Some(n)) = (head.current_run.as_mut(), head.seqs.run) {
            if let Some(r) = rev(TaskPart::Run, seq(n), "children_rev") {
                run.facts.children_rev = r;
            }
            if let Some(r) = rev(TaskPart::Run, seq(n), "obligations_rev") {
                run.facts.obligations_rev = r;
            }
            if let (Some(child), Some(c)) = (run.current_child.as_deref_mut(), head.seqs.run_child)
                && let Some(r) = rev(TaskPart::ReviewRun, child_key(n, c), "obligations_rev")
            {
                child.facts.obligations_rev = r;
            }
        }
        if let (Some(plan), Some(n)) = (head.last_plan.as_mut(), head.seqs.plan)
            && let Some(r) = rev(TaskPart::Plan, seq(n), "children_rev")
        {
            plan.facts.children_rev = r;
        }
        if let (Some(attempt), Some(n)) = (head.last_aggregate_attempt.as_mut(), head.seqs.attempt)
            && let Some(r) = rev(TaskPart::AggregateAttempt, seq(n), "children_rev")
        {
            attempt.facts.children_rev = r;
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
        rows.extend(projection_rows(&self.head)?);
        Ok(rows)
    }

    /// Refuses a direct projection edit that renames, restarts, adds or removes a container or
    /// child in place: only the keyed helpers change which container a position holds.
    pub(super) fn ensure_stable_identities(
        base: &TaskHead,
        head: &TaskHead,
        restarted: super::head_mut::Restarted,
    ) -> Result<()> {
        fn same<T, K: PartialEq>(a: Option<&T>, b: Option<&T>, key: impl Fn(&T) -> K) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => key(a) == key(b),
                (None, None) => true,
                _ => false,
            }
        }
        let (old, new) = (&base.seqs, &head.seqs);
        let run = |r: &RunHead| (r.id.clone(), (!restarted.run).then_some(r.started));
        let checks = [
            (
                "the current run",
                old.run != new.run
                    || same(
                        base.current_run.as_deref(),
                        head.current_run.as_deref(),
                        run,
                    ),
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
                        |r| (r.id.clone(), (!restarted.plan_run).then_some(r.started)),
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

type Key = (TaskPart, String);

/// The rows of a head's current containers and children, as `finish` stores them.
pub(super) fn projection_rows(head: &TaskHead) -> Result<Vec<HistoryRow>> {
    let mut rows = Vec::new();
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

/// The revision counters a container row keeps.
fn revision_fields(part: TaskPart) -> &'static [&'static str] {
    match part {
        TaskPart::Run => &["children_rev", "obligations_rev"],
        TaskPart::ReviewRun => &["obligations_rev"],
        TaskPart::Plan | TaskPart::AggregateAttempt => &["children_rev"],
        _ => &[],
    }
}

/// The container counter a stored child document advances when it changes: review rows advance
/// their run's `children_rev`, plan and aggregate reviews their container's, and waits and
/// decisions the `obligations_rev` of the run or review child that owns them.
fn guarded_by(part: TaskPart, key: &str) -> Option<(Key, &'static str)> {
    let mut segments = key.split('/');
    let first = segments.next()?.to_owned();
    match part {
        TaskPart::ReviewRun => Some(((TaskPart::Run, first), "children_rev")),
        TaskPart::PlanReview => Some(((TaskPart::Plan, first), "children_rev")),
        TaskPart::AggregateReview => Some(((TaskPart::AggregateAttempt, first), "children_rev")),
        TaskPart::RunObligation => {
            let owner = segments.next()?;
            if owner == seq(SELF) {
                Some(((TaskPart::Run, first), "obligations_rev"))
            } else {
                Some((
                    (TaskPart::ReviewRun, format!("{first}/{owner}")),
                    "obligations_rev",
                ))
            }
        }
        _ => None,
    }
}

fn facts_rev(row: &HistoryRow, field: &str) -> u64 {
    row.doc
        .get(FACTS)
        .and_then(|facts| facts.get(field))
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0)
}

fn set_rev(row: &mut HistoryRow, field: &str, rev: u64) {
    if let Some(facts) = row
        .doc
        .get_mut(FACTS)
        .and_then(serde_json::Value::as_object_mut)
    {
        facts.insert(field.to_owned(), format!("{rev:020}").into());
    }
}

impl TaskHeadMut {
    /// Restarts the current coding run's clock in place (a launch's startup grace).
    ///
    /// # Errors
    /// Refuses a task without a current run.
    pub fn restart_current_run(&mut self, at: jiff::Timestamp) -> Result<()> {
        let run = self
            .head
            .current_run
            .as_mut()
            .context("no current run to restart")?;
        run.started = at;
        self.restarted.run = true;
        Ok(())
    }

    /// Restarts the last planning run's clock in place (a planner revision's startup grace).
    ///
    /// # Errors
    /// Refuses a task without a planning run.
    pub fn restart_last_plan_run(&mut self, at: jiff::Timestamp) -> Result<()> {
        let run = self
            .head
            .last_plan_run
            .as_mut()
            .context("no planning run to restart")?;
        run.started = at;
        self.restarted.plan_run = true;
        Ok(())
    }
}
