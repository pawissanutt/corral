//! Head mutations (plan T-253 section 4, "TaskHeadMut"): the task's own fields and the current
//! projections are mutable, history only grows (a new container demotes the current one, which
//! is folded into the summary), and nested history changes only through the child mutations in
//! `head_children`. `finish` turns the result into the rows one `UpdateTask` writes.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::codec::{
    FACTS, Rows, SELF, attempt_rows, child_key, ordinal_key, plan_review_tokens, plan_rows,
    plan_run_rows, run_tokens, seq, split_run,
};
use super::head::{TaskHead, projection};
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead, TaskHeadFields};
use crate::backend::{HistoryRow, TaskPart};
use crate::container_facts::{PaneRef, herdr_panes, own_panes};
use crate::{
    AggregateAttempt, AggregateReview, ApprovedScopeRevision, PlanRecord, PlanRun, Run, TaskComment,
};

/// A container row demoted by this mutation, written with `current` cleared.
#[derive(Clone)]
pub(crate) enum Demoted {
    Run(usize, RunHead),
    RunChild(usize, usize, RunHead),
    PlanRun(usize, PlanRunHead),
    Plan(usize, PlanHead),
    PlanReview(usize, usize, PlanReviewHead),
    Attempt(usize, AttemptHead),
    AggregateReview(usize, usize, AggregateReview),
}

/// The mutable head of one `update_head` call.
pub struct TaskHeadMut {
    pub(crate) head: TaskHead,
    /// The head as read: decides which current container rows changed.
    pub(crate) base: Box<TaskHead>,
    /// The head as of the last fact adjustment: direct projection edits since then still owe
    /// their fact deltas.
    pub(crate) seen: Box<TaskHead>,
    pub(crate) rows: BTreeMap<(TaskPart, String), Option<HistoryRow>>,
    pub(crate) demoted: Vec<Demoted>,
    pub(crate) usage_dirty: bool,
    /// Set when a write rewrites or deletes a history row, which only the full form may do.
    pub(crate) history_edit: bool,
    /// The stored rows this mutation read from its view, by key: `finish` compares the final
    /// write set against them without reading them again.
    pub(crate) originals: std::cell::RefCell<BTreeMap<(TaskPart, String), HistoryRow>>,
    /// Set when this mutation appends a scope revision, which changes the contract digest.
    pub(crate) scope_history_changed: bool,
    /// The head fields and scope revision count the contract revision was last settled for.
    pub(crate) contract_settled: Option<(TaskHeadFields, u64)>,
    /// Set by [`Self::restart_current_run`] and [`Self::restart_last_plan_run`]: the only
    /// in-place change of a current container's start.
    pub(crate) restarted: Restarted,
}

/// Which current containers this mutation restarted the clock of.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Restarted {
    pub(crate) run: bool,
    pub(crate) plan_run: bool,
}

impl Deref for TaskHeadMut {
    type Target = TaskHeadFields;

    fn deref(&self) -> &TaskHeadFields {
        &self.head.task
    }
}

impl DerefMut for TaskHeadMut {
    fn deref_mut(&mut self) -> &mut TaskHeadFields {
        &mut self.head.task
    }
}

pub(crate) fn with_facts<T: serde::Serialize>(
    head: &T,
    facts: &crate::ContainerFacts,
) -> Result<Value> {
    let mut doc = serde_json::to_value(head)?;
    if let Some(map) = doc.as_object_mut() {
        map.insert(FACTS.to_owned(), serde_json::to_value(facts)?);
    }
    Ok(doc)
}

fn history_row(
    part: TaskPart,
    key: String,
    current: bool,
    open: bool,
    doc: Value,
    index: Vec<String>,
) -> HistoryRow {
    HistoryRow {
        part,
        key,
        current,
        open,
        doc,
        index,
    }
}

pub(crate) fn run_row(
    run_seq: usize,
    child: usize,
    run: &RunHead,
    current: bool,
) -> Result<HistoryRow> {
    let (part, key) = if child == SELF {
        (TaskPart::Run, seq(run_seq))
    } else {
        (TaskPart::ReviewRun, child_key(run_seq, child))
    };
    Ok(history_row(
        part,
        key,
        current,
        run.ended.is_none(),
        with_facts(run, &run.facts)?,
        run_tokens(&run.id, child, run.review.as_ref(), run.outcome),
    ))
}

pub(crate) fn plan_run_row(n: usize, run: &PlanRunHead, current: bool) -> Result<HistoryRow> {
    Ok(history_row(
        TaskPart::PlanRun,
        seq(n),
        current,
        run.ended.is_none(),
        with_facts(run, &run.facts)?,
        vec![format!("id:{}", run.id)],
    ))
}

pub(crate) fn plan_row(n: usize, plan: &PlanHead, current: bool) -> Result<HistoryRow> {
    Ok(history_row(
        TaskPart::Plan,
        seq(n),
        current,
        plan.approval.is_none(),
        with_facts(plan, &plan.facts)?,
        vec![format!("id:{}", plan.id)],
    ))
}

pub(crate) fn plan_review_row(
    plan: usize,
    n: usize,
    review: &PlanReviewHead,
    current: bool,
) -> Result<HistoryRow> {
    Ok(history_row(
        TaskPart::PlanReview,
        child_key(plan, n),
        current,
        review.verdict.is_none(),
        serde_json::to_value(review)?,
        plan_review_tokens(&review.id, &review.reviewer, review.verdict),
    ))
}

pub(crate) fn attempt_row(n: usize, attempt: &AttemptHead, current: bool) -> Result<HistoryRow> {
    Ok(history_row(
        TaskPart::AggregateAttempt,
        seq(n),
        current,
        attempt.acceptance.is_none(),
        with_facts(attempt, &attempt.facts)?,
        vec![format!("id:{}", attempt.id)],
    ))
}

pub(crate) fn aggregate_review_row(
    attempt: usize,
    n: usize,
    review: &AggregateReview,
    current: bool,
) -> Result<HistoryRow> {
    Ok(history_row(
        TaskPart::AggregateReview,
        child_key(attempt, n),
        current,
        review.pending(),
        serde_json::to_value(review)?,
        vec![format!("id:{}", review.id)],
    ))
}

pub(crate) fn pane_row(pane: &PaneRef) -> HistoryRow {
    let mut index = Vec::new();
    if !pane.pane.is_empty() {
        index.push(format!("pane:{}", pane.pane));
    }
    if let Some(workspace) = &pane.workspace {
        index.push(format!("workspace:{workspace}"));
    }
    history_row(
        TaskPart::PaneRef,
        pane.key(),
        false,
        false,
        serde_json::json!({"socket": pane.socket, "workspace": pane.workspace}),
        index,
    )
}

pub(crate) fn add_count(map: &mut BTreeMap<String, u64>, key: &str, delta: i64) {
    let entry = map.entry(key.to_owned()).or_default();
    *entry = entry.saturating_add_signed(delta);
    if *entry == 0 {
        map.remove(key);
    }
}

pub(crate) fn add_panes(map: &mut BTreeMap<String, u64>, panes: &[PaneRef], sign: i64) {
    for pane in panes {
        add_count(map, &pane.socket, sign);
    }
}

pub(crate) fn run_own_panes(run_seq: usize, child: usize, run: &RunHead) -> Vec<PaneRef> {
    own_panes(
        &format!("run:{}", child_key(run_seq, child)),
        run.herdr.as_ref(),
        run.gate.as_ref(),
    )
}

pub(crate) fn changes_unsuperseded(child: &RunHead) -> bool {
    child.outcome != Some(crate::RunOutcome::Superseded)
        && child
            .review
            .as_ref()
            .is_some_and(|r| r.verdict == Some(crate::Verdict::Changes))
}

fn split_rows(
    fill: impl FnOnce(&mut Rows) -> Result<()>,
) -> Result<BTreeMap<(TaskPart, String), HistoryRow>> {
    let mut rows = Rows::new();
    fill(&mut rows)?;
    Ok(rows.0)
}

impl TaskHeadMut {
    pub(crate) fn new(head: TaskHead) -> TaskHeadMut {
        TaskHeadMut {
            base: Box::new(head.clone()),
            seen: Box::new(head.clone()),
            head,
            rows: BTreeMap::new(),
            demoted: Vec::new(),
            usage_dirty: false,
            history_edit: false,
            originals: std::cell::RefCell::default(),
            scope_history_changed: false,
            contract_settled: None,
            restarted: Restarted::default(),
        }
    }

    /// The head as mutated so far.
    pub fn head(&self) -> &TaskHead {
        &self.head
    }

    pub fn current_run_mut(&mut self) -> Option<&mut RunHead> {
        self.head.current_run.as_deref_mut()
    }

    pub fn last_plan_run_mut(&mut self) -> Option<&mut PlanRunHead> {
        self.head.last_plan_run.as_deref_mut()
    }

    pub fn last_plan_mut(&mut self) -> Option<&mut PlanHead> {
        self.head.last_plan.as_deref_mut()
    }

    pub fn last_aggregate_attempt_mut(&mut self) -> Option<&mut AttemptHead> {
        self.head.last_aggregate_attempt.as_deref_mut()
    }

    pub(crate) fn put(&mut self, row: HistoryRow) {
        self.rows.insert((row.part, row.key.clone()), Some(row));
    }

    pub(crate) fn delete(&mut self, part: TaskPart, key: String) {
        self.history_edit = true;
        self.rows.insert((part, key), None);
    }

    /// Queues every row of a split container except its own row, which `finish` writes from
    /// the projection.
    fn put_nested(
        &mut self,
        rows: BTreeMap<(TaskPart, String), HistoryRow>,
        own: (TaskPart, &str),
    ) {
        for ((part, key), row) in rows {
            if (part, key.as_str()) != own {
                self.put(row);
            }
        }
    }

    fn next_run_seq(&self) -> usize {
        usize::try_from(self.head.summary.runs).unwrap_or(usize::MAX)
            + usize::from(self.head.current_run.is_some())
            + 1
    }

    /// Starts `run` as the current coding run; the previous one becomes a history row and is
    /// folded into the summary.
    pub fn start_run(&mut self, run: Run) -> Result<()> {
        self.adjust_projections()?;
        self.head.summary.current_artifacts = Default::default();
        // A pre-registered run id is legal. Its scalar facts must not be inferred from the
        // previous current run.
        for artifact in self.head.artifacts_of(&run.id)? {
            self.head.summary.current_artifacts.fold(&artifact);
        }
        for row in self
            .rows
            .values()
            .flatten()
            .filter(|row| row.part == TaskPart::Artifact)
        {
            let artifact: crate::Artifact = serde_json::from_value(row.doc.clone())?;
            if artifact.run == run.id {
                self.head.summary.current_artifacts.fold(&artifact);
            }
        }
        // The outgoing run is folded into the summary now, so its usage facts must be final.
        if self.usage_dirty {
            self.recompute_usage()?;
            self.usage_dirty = false;
        }
        let n = self.next_run_seq();
        if let (Some(old), Some(old_seq)) = (self.head.current_run.take(), self.head.seqs.run) {
            // Its current child may exist only in the projection; it stays its run's last child.
            if let (Some(child), Some(c)) = (old.current_child.as_deref(), self.head.seqs.run_child)
            {
                self.put(run_row(old_seq, c, child, true)?);
            }
            self.head.summary.fold_run_head(&old);
            self.demoted.push(Demoted::Run(old_seq, *old));
        }
        let rows =
            split_rows(|rows| split_run(rows, n, SELF, &run, serde_json::to_value(&run)?, true))?;
        let own = rows
            .get(&(TaskPart::Run, seq(n)))
            .context("the started run has no row")?
            .clone();
        let (mut head, facts): (RunHead, _) = projection(&own.doc)?;
        head.facts = facts;
        let child = run.review_runs.len();
        if child > 0 {
            let row = rows
                .get(&(TaskPart::ReviewRun, child_key(n, child)))
                .context("the started run's current child has no row")?;
            let (mut current, facts): (RunHead, _) = projection(&row.doc)?;
            current.facts = facts;
            head.current_child = Some(Box::new(current));
            self.head.seqs.run_child = Some(child);
        } else {
            self.head.seqs.run_child = None;
        }
        let child_key_own = child_key(n, child);
        for ((part, key), row) in rows {
            let own_row = (part == TaskPart::Run && key == seq(n))
                || (part == TaskPart::ReviewRun && child > 0 && key == child_key_own);
            if !own_row {
                self.put(row);
            }
        }
        self.put_task_panes(&split_panes_of_run(n, &run));
        self.head.current_run = Some(Box::new(head));
        self.head.seqs.run = Some(n);
        *self.seen = self.head.clone();
        Ok(())
    }

    fn put_task_panes(&mut self, panes: &[PaneRef]) {
        for pane in panes {
            self.put(pane_row(pane));
        }
    }

    pub fn start_plan_run(&mut self, run: PlanRun) -> Result<()> {
        self.adjust_projections()?;
        let n = usize::try_from(self.head.summary.plan_runs).unwrap_or(usize::MAX)
            + usize::from(self.head.last_plan_run.is_some())
            + 1;
        if let (Some(old), Some(old_seq)) =
            (self.head.last_plan_run.take(), self.head.seqs.plan_run)
        {
            self.head.summary.fold_plan_run_head(&old);
            self.demoted.push(Demoted::PlanRun(old_seq, *old));
        }
        let rows =
            split_rows(|rows| plan_run_rows(rows, n, &run, serde_json::to_value(&run)?, true))?;
        let own = rows
            .get(&(TaskPart::PlanRun, seq(n)))
            .context("the plan run has no row")?;
        let (mut head, facts): (PlanRunHead, _) = projection(&own.doc)?;
        head.facts = facts;
        self.put_nested(rows, (TaskPart::PlanRun, &seq(n)));
        self.put_task_panes(&crate::container_facts::plan_run_panes(
            &format!("plan_run:{}", seq(n)),
            &run,
        ));
        self.head.last_plan_run = Some(Box::new(head));
        self.head.seqs.plan_run = Some(n);
        *self.seen = self.head.clone();
        Ok(())
    }

    pub fn push_plan(&mut self, plan: PlanRecord) -> Result<()> {
        self.adjust_projections()?;
        let n = usize::try_from(self.head.summary.plans).unwrap_or(usize::MAX)
            + usize::from(self.head.last_plan.is_some())
            + 1;
        if let (Some(old), Some(old_seq)) = (self.head.last_plan.take(), self.head.seqs.plan) {
            // Its current review may exist only in the projection; it stays the last review.
            if let (Some(review), Some(r)) = (&old.current_review, self.head.seqs.plan_review) {
                self.put(plan_review_row(old_seq, r, review, true)?);
            }
            self.head.summary.fold_plan_head(&old);
            self.demoted.push(Demoted::Plan(old_seq, *old));
        }
        let rows =
            split_rows(|rows| plan_rows(rows, n, &plan, serde_json::to_value(&plan)?, true))?;
        let own = rows
            .get(&(TaskPart::Plan, seq(n)))
            .context("the plan has no row")?;
        let (mut head, facts): (PlanHead, _) = projection(&own.doc)?;
        head.facts = facts;
        let reviews = plan.reviews.len();
        if reviews > 0 {
            let row = rows
                .get(&(TaskPart::PlanReview, child_key(n, reviews)))
                .context("the plan's current review has no row")?;
            head.current_review = Some(serde_json::from_value(row.doc.clone())?);
        }
        self.head.seqs.plan_review = (reviews > 0).then_some(reviews);
        let current_review = child_key(n, reviews);
        for ((part, key), row) in rows {
            let own = (part == TaskPart::Plan && key == seq(n))
                || (part == TaskPart::PlanReview && reviews > 0 && key == current_review);
            if !own {
                self.put(row);
            }
        }
        for (r, review) in plan.reviews.iter().enumerate() {
            self.put_task_panes(&herdr_panes(
                &format!("plan_review:{}", child_key(n, r + 1)),
                "plan_review",
                review.herdr.as_ref(),
            ));
        }
        self.head.last_plan = Some(Box::new(head));
        self.head.seqs.plan = Some(n);
        *self.seen = self.head.clone();
        Ok(())
    }

    pub fn push_aggregate_attempt(&mut self, attempt: AggregateAttempt) -> Result<()> {
        self.adjust_projections()?;
        let n = usize::try_from(self.head.summary.aggregate_attempts).unwrap_or(usize::MAX)
            + usize::from(self.head.last_aggregate_attempt.is_some())
            + 1;
        if let (Some(old), Some(old_seq)) = (
            self.head.last_aggregate_attempt.take(),
            self.head.seqs.attempt,
        ) {
            if let (Some(review), Some(r)) = (&old.current_review, self.head.seqs.attempt_review) {
                self.put(aggregate_review_row(old_seq, r, review, true)?);
            }
            self.head.summary.fold_attempt_head(&old);
            self.demoted.push(Demoted::Attempt(old_seq, *old));
        }
        let rows = split_rows(|rows| {
            attempt_rows(rows, n, &attempt, serde_json::to_value(&attempt)?, true)
        })?;
        let own = rows
            .get(&(TaskPart::AggregateAttempt, seq(n)))
            .context("the aggregate attempt has no row")?;
        let (mut head, facts): (AttemptHead, _) = projection(&own.doc)?;
        head.facts = facts;
        let reviews = attempt.reviews.len();
        head.current_review = attempt.reviews.last().cloned();
        self.head.seqs.attempt_review = (reviews > 0).then_some(reviews);
        let current_review = child_key(n, reviews);
        for ((part, key), row) in rows {
            let own = (part == TaskPart::AggregateAttempt && key == seq(n))
                || (part == TaskPart::AggregateReview && reviews > 0 && key == current_review);
            if !own {
                self.put(row);
            }
        }
        for (r, review) in attempt.reviews.iter().enumerate() {
            self.put_task_panes(&herdr_panes(
                &format!("aggregate_review:{}", child_key(n, r + 1)),
                "aggregate_review",
                review.herdr.as_ref(),
            ));
        }
        self.head.last_aggregate_attempt = Some(Box::new(head));
        self.head.seqs.attempt = Some(n);
        *self.seen = self.head.clone();
        Ok(())
    }

    pub fn add_comment(&mut self, comment: TaskComment) -> Result<()> {
        self.head.summary.comments += 1;
        let n = usize::try_from(self.head.summary.comments)?;
        self.put(history_row(
            TaskPart::Comment,
            ordinal_key(n),
            false,
            false,
            serde_json::to_value(comment)?,
            Vec::new(),
        ));
        Ok(())
    }

    /// Appends a registration and folds its scalar facts without loading earlier artifacts.
    pub fn add_artifact(&mut self, artifact: crate::Artifact) -> Result<()> {
        self.head.summary.artifacts += 1;
        self.head.summary.kept_artifacts +=
            u64::from(artifact.tier() == crate::ArtifactTier::Keep && artifact.purged_at.is_none());
        if self
            .head
            .current_run
            .as_ref()
            .is_some_and(|r| r.id == artifact.run)
        {
            self.head.summary.current_artifacts.fold(&artifact);
        }
        self.put(history_row(
            TaskPart::Artifact,
            ordinal_key(usize::try_from(self.head.summary.artifacts)?),
            false,
            false,
            serde_json::to_value(&artifact)?,
            super::head_api::artifact_indexes(&artifact),
        ));
        Ok(())
    }

    pub fn push_scope_revision(&mut self, revision: ApprovedScopeRevision) -> Result<()> {
        self.scope_history_changed = true;
        self.head.summary.scope_revisions += 1;
        let n = usize::try_from(self.head.summary.scope_revisions)?;
        self.put(history_row(
            TaskPart::ScopeRevision,
            seq(n),
            false,
            false,
            serde_json::to_value(revision)?,
            Vec::new(),
        ));
        Ok(())
    }

    pub(crate) fn current_run_seq(&self, run: &str) -> Result<usize> {
        match (&self.head.current_run, self.head.seqs.run) {
            (Some(current), Some(seq)) if current.id == run => Ok(seq),
            _ => bail!("run {run} is not the current run of {}", self.head.task.id),
        }
    }

    /// Adjusts facts and pane rows for edits the caller made to projections directly, by
    /// comparing each current projection with the one this mutation started from.
    pub(crate) fn adjust_projections(&mut self) -> Result<()> {
        let base = self.seen.clone();
        Self::ensure_stable_identities(&base, &self.head, self.restarted)?;
        if let (Some(run), Some(old), Some(seq)) = (
            self.head.current_run.as_mut(),
            base.current_run.as_ref(),
            self.head.seqs.run,
        ) && base.seqs.run == Some(seq)
        {
            let mut pane_writes = Vec::new();
            if run.harness != old.harness {
                if let Some(h) = &old.harness {
                    add_count(&mut run.facts.harnesses, h, -1);
                }
                if let Some(h) = &run.harness {
                    add_count(&mut run.facts.harnesses, h, 1);
                }
            }
            let (before, after) = (run_own_panes(seq, SELF, old), run_own_panes(seq, SELF, run));
            if before != after {
                add_panes(&mut run.facts.pane_refs, &before, -1);
                add_panes(&mut run.facts.pane_refs, &after, 1);
                pane_writes.push((before, after));
            }
            if run.usage != old.usage {
                self.usage_dirty = true;
            }
            if let (Some(child), Some(old_child), Some(n)) = (
                run.current_child.as_deref_mut(),
                old.current_child.as_deref(),
                self.head.seqs.run_child,
            ) && base.seqs.run_child == Some(n)
                && serde_json::to_value(&*child)? != serde_json::to_value(old_child)?
            {
                run.facts.children_rev += 1;
                let open = |c: &RunHead| i64::from(c.ended.is_none());
                run.facts.open_children = run
                    .facts
                    .open_children
                    .saturating_add_signed(open(child) - open(old_child));
                let changes = |c: &RunHead| i64::from(changes_unsuperseded(c));
                run.facts.changes_unsuperseded = run
                    .facts
                    .changes_unsuperseded
                    .saturating_add_signed(changes(child) - changes(old_child));
                if child.harness != old_child.harness {
                    for (harness, sign) in [(&old_child.harness, -1), (&child.harness, 1)] {
                        if let Some(h) = harness {
                            add_count(&mut run.facts.harnesses, h, sign);
                            add_count(&mut child.facts.harnesses, h, sign);
                        }
                    }
                }
                let (before, after) = (
                    run_own_panes(seq, n, old_child),
                    run_own_panes(seq, n, child),
                );
                if before != after {
                    for facts in [&mut run.facts.pane_refs, &mut child.facts.pane_refs] {
                        add_panes(facts, &before, -1);
                        add_panes(facts, &after, 1);
                    }
                    pane_writes.push((before, after));
                }
                if child.usage != old_child.usage {
                    self.usage_dirty = true;
                }
            }
            for (before, after) in pane_writes {
                self.write_panes(&before, &after);
            }
        }
        if let (Some(run), Some(old)) = (
            self.head.last_plan_run.as_mut(),
            base.last_plan_run.as_ref(),
        ) && self.head.seqs.plan_run == base.seqs.plan_run
            && run.herdr != old.herdr
        {
            let owner = format!("plan_run:{}", seq(self.head.seqs.plan_run.unwrap_or(0)));
            let before = herdr_panes(&owner, "plan_run", old.herdr.as_ref());
            let after = herdr_panes(&owner, "plan_run", run.herdr.as_ref());
            add_panes(&mut run.facts.pane_refs, &before, -1);
            add_panes(&mut run.facts.pane_refs, &after, 1);
            self.write_panes(&before, &after);
        }
        if let (Some(plan), Some(old)) = (self.head.last_plan.as_mut(), base.last_plan.as_ref())
            && self.head.seqs.plan == base.seqs.plan
            && self.head.seqs.plan_review == base.seqs.plan_review
            && let (Some(review), Some(old_review)) = (&plan.current_review, &old.current_review)
            && review != old_review
        {
            plan.facts.children_rev += 1;
            let changes =
                |r: &PlanReviewHead| i64::from(r.verdict == Some(crate::Verdict::Changes));
            let pending = |r: &PlanReviewHead| i64::from(r.verdict.is_none());
            plan.facts.changes = plan
                .facts
                .changes
                .saturating_add_signed(changes(review) - changes(old_review));
            plan.facts.pending_children = plan
                .facts
                .pending_children
                .saturating_add_signed(pending(review) - pending(old_review));
            let owner = format!(
                "plan_review:{}",
                child_key(
                    self.head.seqs.plan.unwrap_or(0),
                    self.head.seqs.plan_review.unwrap_or(0)
                )
            );
            let before = herdr_panes(&owner, "plan_review", old_review.herdr.as_ref());
            let after = herdr_panes(&owner, "plan_review", review.herdr.as_ref());
            add_panes(&mut plan.facts.pane_refs, &before, -1);
            add_panes(&mut plan.facts.pane_refs, &after, 1);
            self.write_panes(&before, &after);
        }
        if let (Some(attempt), Some(old)) = (
            self.head.last_aggregate_attempt.as_mut(),
            base.last_aggregate_attempt.as_ref(),
        ) && self.head.seqs.attempt == base.seqs.attempt
            && self.head.seqs.attempt_review == base.seqs.attempt_review
            && let (Some(review), Some(old_review)) = (&attempt.current_review, &old.current_review)
            && review != old_review
        {
            attempt.facts.children_rev += 1;
            let pending = |r: &AggregateReview| i64::from(r.pending());
            attempt.facts.pending_children = attempt
                .facts
                .pending_children
                .saturating_add_signed(pending(review) - pending(old_review));
            let owner = format!(
                "aggregate_review:{}",
                child_key(
                    self.head.seqs.attempt.unwrap_or(0),
                    self.head.seqs.attempt_review.unwrap_or(0)
                )
            );
            let before = herdr_panes(&owner, "aggregate_review", old_review.herdr.as_ref());
            let after = herdr_panes(&owner, "aggregate_review", review.herdr.as_ref());
            add_panes(&mut attempt.facts.pane_refs, &before, -1);
            add_panes(&mut attempt.facts.pane_refs, &after, 1);
            self.write_panes(&before, &after);
        }
        *self.seen = self.head.clone();
        Ok(())
    }

    pub(crate) fn write_panes(&mut self, before: &[PaneRef], after: &[PaneRef]) {
        for pane in before
            .iter()
            .filter(|p| !after.iter().any(|a| a.key() == p.key()))
        {
            self.delete(TaskPart::PaneRef, pane.key());
        }
        for pane in after.iter().filter(|p| !before.contains(p)) {
            self.put(pane_row(pane));
        }
    }

    pub(crate) fn ensure_unchanged_identity(old: &Run, new: &Run) -> Result<()> {
        ensure!(
            old.id == new.id && old.started == new.started,
            "a child edit may not change its id or start"
        );
        Ok(())
    }
}

pub(crate) fn split_panes_of_run(n: usize, run: &Run) -> Vec<PaneRef> {
    let mut panes = crate::container_facts::run_panes(&format!("run:{}", child_key(n, SELF)), run);
    for (c, child) in run.review_runs.iter().enumerate() {
        panes.extend(crate::container_facts::run_panes(
            &format!("run:{}", child_key(n, c + 1)),
            child,
        ));
    }
    panes
}
