//! Child mutations of a head (plan T-253 section 4, "Child mutations"): pushes demote the current
//! child and fold the container's facts; keyed edits read only the edited child's own rows and
//! adjust facts by its delta; ids, ordinals and starts never change.

use super::RunHead;
use super::assemble::{TaskRows, run_value};
use super::codec::{Rows, SELF, child_key, cursor_token, seq, split_run};
use super::head::{last_ordinal, projection};
use super::head_mut::{
    Demoted, TaskHeadMut, add_count, add_panes, changes_unsuperseded, run_own_panes,
};
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::container_facts::{CostTotals, run_panes};
use crate::inbox::{Recipient, SessionCursor};
use crate::work::Counter;
use crate::{
    CheckpointMeta, CheckpointRecord, ContainerFacts, DecisionRecord, ProfileSwitch, Run, Usage,
    UsageEntry, UsageTotals, WaitRecord, WaitState,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeMap;

const RUN_NESTED: [TaskPart; 5] = [
    TaskPart::RunUsage,
    TaskPart::RunCheckpoint,
    TaskPart::RunSwitch,
    TaskPart::RunObligation,
    TaskPart::AgentRecord,
];

type RowMap = BTreeMap<(TaskPart, String), HistoryRow>;
impl TaskHeadMut {
    /// Rows matching `q` as this mutation would store them: the view's, overlaid with the rows
    /// already queued.
    pub(crate) fn rows_now(&self, q: &RowQuery, counter: Counter) -> Result<Vec<HistoryRow>> {
        self.overlay(q, self.head.query(q, u32::MAX, counter)?)
    }

    /// [`Self::rows_now`] of rows that all belong to run `run`.
    pub(crate) fn rows_now_for_run(
        &self,
        q: &RowQuery,
        counter: Counter,
        run: &str,
    ) -> Result<Vec<HistoryRow>> {
        self.overlay(q, self.head.query_for_run(q, counter, run)?)
    }

    fn overlay(&self, q: &RowQuery, stored: Vec<HistoryRow>) -> Result<Vec<HistoryRow>> {
        {
            let mut originals = self.originals.borrow_mut();
            for row in &stored {
                originals.insert((row.part, row.key.clone()), row.clone());
            }
        }
        let mut found: BTreeMap<String, HistoryRow> = stored
            .into_iter()
            .map(|row| (row.key.clone(), row))
            .collect();
        let pending = self.pending_rows()?;
        let queued = self
            .rows
            .iter()
            .map(|((part, key), row)| (*part, key, row.as_ref()));
        let held = pending.iter().map(|row| (row.part, &row.key, Some(row)));
        for (part, key, row) in queued.chain(held) {
            if part != q.part {
                continue;
            }
            match row {
                Some(row) if q.matches(row) => {
                    found.insert(key.clone(), row.clone());
                }
                _ => {
                    found.remove(key);
                }
            }
        }
        let mut rows: Vec<HistoryRow> = found.into_values().collect();
        if q.newest_first {
            rows.reverse();
        }
        Ok(rows)
    }

    /// The largest ordinal under `prefix`, stored or queued, reading only the newest stored row.
    fn last_key(&self, part: TaskPart, prefix: &str) -> Result<usize> {
        let stored = self
            .head
            .query(
                &RowQuery::under(part, prefix).newest_first(),
                1,
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .map(|row| last_ordinal(&row.key))
            .transpose()?
            .unwrap_or(0);
        let mut last = stored;
        for (row_part, key) in self.rows.keys() {
            if *row_part == part && key.starts_with(prefix) {
                last = last.max(last_ordinal(key)?);
            }
        }
        Ok(last)
    }

    fn current_run_facts(&mut self, run: &str) -> Result<(usize, &mut ContainerFacts)> {
        let seq = self.current_run_seq(run)?;
        let run = self.head.current_run.as_mut().context("no current run")?;
        Ok((seq, &mut run.facts))
    }

    /// Every stored row of review child `n` of run `s`: its own row, nested rows, inbox rows
    /// and the checkpoint payloads it references.
    fn child_rows(&self, s: usize, n: usize, own: HistoryRow) -> Result<RowMap> {
        let owner = child_key(s, n);
        let mut rows = RowMap::new();
        rows.insert((own.part, own.key.clone()), own);
        for part in RUN_NESTED {
            for row in self.rows_now(
                &RowQuery::under(part, format!("{owner}/")),
                Counter::NestedRowDecodes,
            )? {
                rows.insert((row.part, row.key.clone()), row);
            }
        }
        for part in [TaskPart::InboxEffect, TaskPart::ObserveCursor] {
            for row in self.rows_now(
                &RowQuery::under(part, format!("run:{owner}/")),
                Counter::NestedRowDecodes,
            )? {
                rows.insert((row.part, row.key.clone()), row);
            }
        }
        let digests: Vec<String> = rows
            .values()
            .filter(|row| row.part == TaskPart::RunCheckpoint)
            .filter_map(|row| Some(row.doc.get("payload_digest")?.as_str()?.to_owned()))
            .collect();
        for digest in digests {
            for row in self.rows_now(
                &RowQuery::key(TaskPart::CheckpointPayload, digest),
                Counter::NestedRowDecodes,
            )? {
                rows.insert((row.part, row.key.clone()), row);
            }
        }
        Ok(rows)
    }

    /// Queues the writes turning `old` into `new`; a payload is deleted with its last
    /// referencing checkpoint meta and stays while a meta outside `old` still names it.
    pub(crate) fn write_diff(&mut self, old: &RowMap, new: &RowMap) -> Result<()> {
        for (key, row) in new {
            if old.get(key) != Some(row) {
                if old.get(key).is_some_and(|stored| !stored.current) {
                    self.history_edit = true;
                }
                self.put(row.clone());
            }
        }
        for (part, key) in old.keys() {
            if new.contains_key(&(*part, key.clone())) {
                continue;
            }
            if *part == TaskPart::CheckpointPayload && self.payload_kept(key, old, new)? {
                continue;
            }
            self.delete(*part, key.clone());
        }
        Ok(())
    }

    /// Whether a meta that survives this diff (outside `old`, or kept in `new`) names `digest`.
    fn payload_kept(&self, digest: &str, old: &RowMap, new: &RowMap) -> Result<bool> {
        let refs = self.rows_now(
            &RowQuery::part(TaskPart::RunCheckpoint).with_index(format!("payload:{digest}")),
            Counter::NestedRowDecodes,
        )?;
        let removed = |row: &HistoryRow| {
            let key = (row.part, row.key.clone());
            old.contains_key(&key) && !new.contains_key(&key)
        };
        Ok(refs.iter().any(|row| !removed(row))
            || new
                .values()
                .any(|row| row.doc.get("payload_digest").and_then(Value::as_str) == Some(digest)))
    }

    pub(super) fn adjust_child(facts: &mut ContainerFacts, child: &RunHead, sign: i64) {
        facts.open_children = facts
            .open_children
            .saturating_add_signed(sign * i64::from(child.ended.is_none()));
        facts.changes_unsuperseded = facts
            .changes_unsuperseded
            .saturating_add_signed(sign * i64::from(changes_unsuperseded(child)));
        if let Some(harness) = &child.harness {
            add_count(&mut facts.harnesses, harness, sign);
        }
    }

    /// Pushes a review child onto the current run `run`; the previous current child becomes a
    /// history row.
    pub fn push_review_child(&mut self, run: &str, child: Run) -> Result<()> {
        self.adjust_projections()?;
        let s = self.current_run_seq(run)?;
        let exists = self.rows_now(
            &RowQuery::under(TaskPart::ReviewRun, format!("{}/", seq(s)))
                .with_index(format!("id:{}", child.id)),
            Counter::NestedRowDecodes,
        )?;
        ensure!(
            exists.is_empty(),
            "review child {} already exists on {run}",
            child.id
        );
        let current = self.head.current_run.as_mut().context("no current run")?;
        let n = usize::try_from(current.facts.children)? + 1;
        if let (Some(old), Some(c)) = (current.current_child.take(), self.head.seqs.run_child) {
            self.demoted.push(Demoted::RunChild(s, c, *old));
        }
        let mut rows = Rows::new();
        split_run(
            &self.head.id,
            &mut rows,
            s,
            n,
            &child,
            serde_json::to_value(&child)?,
            true,
        )?;
        let own_key = (TaskPart::ReviewRun, child_key(s, n));
        let own = rows
            .0
            .remove(&own_key)
            .context("the review child has no row")?;
        let (mut head, facts): (RunHead, _) = projection(&own.doc)?;
        head.facts = facts;
        let panes = run_panes(&format!("run:{}", child_key(s, n)), &child);
        let current = self.head.current_run.as_mut().context("no current run")?;
        current.facts.children += 1;
        current.facts.children_rev += 1;
        if n == 1 {
            current.facts.first_child_started = Some(child.started);
        }
        Self::adjust_child(&mut current.facts, &head, 1);
        add_panes(&mut current.facts.pane_refs, &panes, 1);
        current.current_child = Some(Box::new(head));
        self.head.seqs.run_child = Some(n);
        if child.total_usage().is_some() {
            self.usage_dirty = true;
        }
        for row in rows.0.into_values() {
            self.put(row);
        }
        for pane in &panes {
            self.put(super::head_mut::pane_row(pane));
        }
        *self.seen = self.head.clone();
        Ok(())
    }

    /// Edits review child `id` of the current run `run` as a whole `Run`, reading only that
    /// child's own rows.
    pub fn edit_review_child(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut Run) -> Result<()>,
    ) -> Result<()> {
        self.adjust_projections()?;
        let s = self.current_run_seq(run)?;
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
        let current = row.current;
        let old_rows = self.child_rows(s, n, row.clone())?;
        let view = TaskRows::new(old_rows.values());
        let before: Run = serde_json::from_value(run_value(&view, s, n, &row.doc)?)?;
        let mut after = before.clone();
        edit(&mut after)?;
        TaskHeadMut::ensure_unchanged_identity(&before, &after)?;
        let mut rows = Rows::new();
        split_run(
            &self.head.id,
            &mut rows,
            s,
            n,
            &after,
            serde_json::to_value(&after)?,
            current,
        )?;
        super::write::carry_revisions(&old_rows, &mut rows.0);
        let (old_head, _): (RunHead, _) = projection(&row.doc)?;
        let new_row = rows
            .0
            .get(&(TaskPart::ReviewRun, child_key(s, n)))
            .context("the edited child has no row")?
            .clone();
        let (mut new_head, facts): (RunHead, _) = projection(&new_row.doc)?;
        new_head.facts = facts;
        let new_rows: RowMap = rows.0;
        let own = (TaskPart::ReviewRun, child_key(s, n));
        let changed = new_rows.get(&own) != old_rows.get(&own);
        self.write_diff(&old_rows, &new_rows)?;
        let (before_panes, after_panes) = (
            run_own_panes(s, n, &old_head),
            run_own_panes(s, n, &new_head),
        );
        self.write_panes(&before_panes, &after_panes);
        let usage_changed = before.total_usage() != after.total_usage();
        let parent = self.head.current_run.as_mut().context("no current run")?;
        if changed {
            parent.facts.children_rev += 1;
        }
        Self::adjust_child(&mut parent.facts, &old_head, -1);
        Self::adjust_child(&mut parent.facts, &new_head, 1);
        add_panes(&mut parent.facts.pane_refs, &before_panes, -1);
        add_panes(&mut parent.facts.pane_refs, &after_panes, 1);
        if current {
            parent.current_child = Some(Box::new(new_head));
        }
        if usage_changed {
            self.usage_dirty = true;
        }
        *self.seen = self.head.clone();
        Ok(())
    }

    /// [`Self::edit_review_child`] of the current run's current child.
    pub fn edit_current_review_child(
        &mut self,
        run: &str,
        edit: impl FnOnce(&mut Run) -> Result<()>,
    ) -> Result<()> {
        let id = self
            .head
            .current_run
            .as_ref()
            .filter(|current| current.id == run)
            .and_then(|current| current.current_child.as_ref())
            .map(|child| child.id.clone())
            .with_context(|| format!("run {run} has no current review child"))?;
        self.edit_review_child(run, &id, edit)
    }

    pub fn push_checkpoint(&mut self, run: &str, record: CheckpointRecord) -> Result<()> {
        let (s, facts) = self.current_run_facts(run)?;
        let n = facts.checkpoints + 1;
        let meta = CheckpointMeta::of(n, &record)?;
        facts.checkpoints = n;
        facts.latest_checkpoint = Some(meta.clone());
        let payload = serde_json::to_value(&record.checkpoint)?;
        self.put(plain(
            TaskPart::CheckpointPayload,
            meta.payload_digest.clone(),
            payload,
        ));
        let mut row = plain(
            TaskPart::RunCheckpoint,
            format!("{}/{}", child_key(s, SELF), seq(usize::try_from(n)?)),
            serde_json::to_value(&meta)?,
        );
        row.index = vec![
            format!("artifact:{}", record.artifact),
            format!("payload:{}", meta.payload_digest),
        ];
        self.put(row);
        Ok(())
    }

    /// Copies at most two checkpoints into the new current run, sharing their payloads.
    pub fn restore_checkpoints(&mut self, run: &str, records: Vec<CheckpointRecord>) -> Result<()> {
        ensure!(records.len() <= 2, "a run restores at most two checkpoints");
        for record in records {
            self.push_checkpoint(run, record)?;
        }
        Ok(())
    }

    pub fn push_switch(&mut self, run: &str, switch: ProfileSwitch) -> Result<()> {
        let (s, facts) = self.current_run_facts(run)?;
        facts.switches += 1;
        facts.last_switch = Some(switch.clone());
        let n = usize::try_from(facts.switches)?;
        self.put(plain(
            TaskPart::RunSwitch,
            format!("{}/{}", child_key(s, SELF), seq(n)),
            serde_json::to_value(switch)?,
        ));
        Ok(())
    }

    fn obligation_row(
        s: usize,
        kind: &str,
        n: usize,
        doc: Value,
        id: &str,
        receipt: Option<&str>,
        open: bool,
    ) -> HistoryRow {
        let mut row = plain(
            TaskPart::RunObligation,
            format!("{}/{kind}/{}", child_key(s, SELF), seq(n)),
            doc,
        );
        row.open = open;
        row.index = std::iter::once(format!("id:{id}"))
            .chain(std::iter::once(format!("{kind}:id:{id}")))
            .chain(receipt.map(|r| format!("receipt:{r}")))
            .collect();
        if crate::card::card_obligation_visible(kind, &row.doc) {
            row.index.push("card".into());
        }
        row
    }

    fn ensure_new_obligation(&self, s: usize, id: &str) -> Result<()> {
        let found = self.rows_now(
            &RowQuery::under(TaskPart::RunObligation, format!("{}/", child_key(s, SELF)))
                .with_index(format!("id:{id}")),
            Counter::NestedRowDecodes,
        )?;
        ensure!(found.is_empty(), "obligation {id} already exists");
        Ok(())
    }

    pub fn push_wait(&mut self, run: &str, wait: WaitRecord) -> Result<()> {
        let s = self.current_run_seq(run)?;
        self.ensure_new_obligation(s, &wait.request.id)?;
        let parking = crate::ContainerFacts::wait_parking(&self.head.id, &wait);
        let (_, facts) = self.current_run_facts(run)?;
        facts.dependency_waits += parking.0;
        facts.nondependency_waits += parking.1;
        facts.waits += 1;
        facts.obligations_rev += 1;
        facts.pending_waits += u64::from(wait.observation.state == WaitState::Pending);
        facts.open_obligations += u64::from(wait.receipt.is_none());
        let n = usize::try_from(facts.waits)?;
        let receipt = wait.receipt.as_ref().map(|r| r.id.clone());
        self.put(Self::obligation_row(
            s,
            "wait",
            n,
            serde_json::to_value(&wait)?,
            &wait.request.id,
            receipt.as_deref(),
            wait.open(run),
        ));
        Ok(())
    }

    pub fn push_decision(&mut self, run: &str, decision: DecisionRecord) -> Result<()> {
        let s = self.current_run_seq(run)?;
        self.ensure_new_obligation(s, &decision.request.id)?;
        let (_, facts) = self.current_run_facts(run)?;
        facts.decisions += 1;
        facts.obligations_rev += 1;
        facts.open_obligations += u64::from(decision.receipt.is_none());
        if facts.pending_decision.is_none() && decision.pending() {
            facts.pending_decision = Some(decision.clone());
            facts.pending_decision_ordinal = Some(facts.decisions);
        }
        let n = usize::try_from(facts.decisions)?;
        let receipt = decision.receipt.as_ref().map(|r| r.id.clone());
        self.put(Self::obligation_row(
            s,
            "decision",
            n,
            serde_json::to_value(&decision)?,
            &decision.request.id,
            receipt.as_deref(),
            decision.receipt.is_none(),
        ));
        Ok(())
    }

    fn obligation(&self, s: usize, kind: &str, id: &str) -> Result<HistoryRow> {
        self.rows_now(
            &RowQuery::under(
                TaskPart::RunObligation,
                format!("{}/{kind}/", child_key(s, SELF)),
            )
            .with_index(format!("id:{id}")),
            Counter::NestedRowDecodes,
        )?
        .into_iter()
        .next()
        .with_context(|| format!("{kind} {id} not found"))
    }

    pub fn edit_wait(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut WaitRecord) -> Result<()>,
    ) -> Result<()> {
        let s = self.current_run_seq(run)?;
        let row = self.obligation(s, "wait", id)?;
        let before: WaitRecord = serde_json::from_value(row.doc.clone())?;
        let mut after = before.clone();
        edit(&mut after)?;
        ensure!(
            after.request.id == before.request.id,
            "a wait edit may not change its id"
        );
        let new = Self::obligation_row(
            s,
            "wait",
            last_ordinal(&row.key)?,
            serde_json::to_value(&after)?,
            id,
            after.receipt.as_ref().map(|r| r.id.as_str()),
            after.open(run),
        );
        if new == row {
            return Ok(());
        }
        self.history_edit = true;
        let before_parking = crate::ContainerFacts::wait_parking(&self.head.id, &before);
        let after_parking = crate::ContainerFacts::wait_parking(&self.head.id, &after);
        let (_, facts) = self.current_run_facts(run)?;
        facts.dependency_waits = facts
            .dependency_waits
            .saturating_sub(before_parking.0)
            .saturating_add(after_parking.0);
        facts.nondependency_waits = facts
            .nondependency_waits
            .saturating_sub(before_parking.1)
            .saturating_add(after_parking.1);
        let pending = |w: &WaitRecord| i64::from(w.observation.state == WaitState::Pending);
        let open = |w: &WaitRecord| i64::from(w.receipt.is_none());
        facts.pending_waits = facts
            .pending_waits
            .saturating_add_signed(pending(&after) - pending(&before));
        facts.open_obligations = facts
            .open_obligations
            .saturating_add_signed(open(&after) - open(&before));
        facts.obligations_rev += 1;
        self.put(new);
        Ok(())
    }

    pub fn edit_decision(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut DecisionRecord) -> Result<()>,
    ) -> Result<()> {
        let s = self.current_run_seq(run)?;
        let row = self.obligation(s, "decision", id)?;
        let ordinal = u64::try_from(last_ordinal(&row.key)?)?;
        let before: DecisionRecord = serde_json::from_value(row.doc.clone())?;
        let mut after = before.clone();
        edit(&mut after)?;
        ensure!(
            after.request.id == before.request.id,
            "a decision edit may not change its id"
        );
        let new = Self::obligation_row(
            s,
            "decision",
            usize::try_from(ordinal)?,
            serde_json::to_value(&after)?,
            id,
            after.receipt.as_ref().map(|r| r.id.as_str()),
            after.receipt.is_none(),
        );
        if new == row {
            return Ok(());
        }
        self.history_edit = true;
        self.put(new);
        let first_open = self
            .rows_now(
                &RowQuery::under(
                    TaskPart::RunObligation,
                    format!("{}/decision/", child_key(s, SELF)),
                )
                .open(),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next();
        let (_, facts) = self.current_run_facts(run)?;
        let open = |d: &DecisionRecord| i64::from(d.receipt.is_none());
        facts.open_obligations = facts
            .open_obligations
            .saturating_add_signed(open(&after) - open(&before));
        facts.obligations_rev += 1;
        match first_open {
            Some(row) => {
                facts.pending_decision_ordinal = Some(u64::try_from(last_ordinal(&row.key)?)?);
                facts.pending_decision = Some(serde_json::from_value(row.doc)?);
            }
            None => {
                facts.pending_decision = None;
                facts.pending_decision_ordinal = None;
            }
        }
        Ok(())
    }

    /// Resolves containers and children against this mutation, including demoted and queued rows.
    fn inbox_owner_now(&self, recipient: &Recipient) -> Result<String> {
        let head = &self.head;
        let (part, id, current) = match recipient {
            Recipient::Run { run, .. } | Recipient::ReviewRun { run, .. } => (
                TaskPart::Run,
                run,
                head.current_run
                    .as_ref()
                    .filter(|r| &r.id == run)
                    .and(head.seqs.run),
            ),
            Recipient::PlanRun { plan_run, .. } => (
                TaskPart::PlanRun,
                plan_run,
                head.last_plan_run
                    .as_ref()
                    .filter(|r| &r.id == plan_run)
                    .and(head.seqs.plan_run),
            ),
            Recipient::PlanReview { plan, .. } => (
                TaskPart::Plan,
                plan,
                head.last_plan
                    .as_ref()
                    .filter(|p| &p.id == plan)
                    .and(head.seqs.plan),
            ),
            Recipient::Pm => anyhow::bail!("the recipient has no inbox on this task"),
        };
        let key = match current {
            Some(s) => seq(s),
            None => {
                self.rows_now(
                    &RowQuery::part(part).with_index(format!("id:{id}")),
                    Counter::NestedRowDecodes,
                )?
                .into_iter()
                .next()
                .context("the recipient has no inbox on this task")?
                .key
            }
        };
        let (part, review, owner) = match recipient {
            Recipient::Run { .. } => return Ok(format!("run:{key}/{}", seq(SELF))),
            Recipient::PlanRun { .. } => return Ok(format!("plan_run:{key}")),
            Recipient::ReviewRun { review, .. } => (TaskPart::ReviewRun, review, "run"),
            Recipient::PlanReview { review, .. } => (TaskPart::PlanReview, review, "plan_review"),
            Recipient::Pm => anyhow::bail!("the recipient has no inbox on this task"),
        };
        let row = self
            .rows_now(
                &RowQuery::under(part, format!("{key}/")).with_index(format!("id:{review}")),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next()
            .context("the recipient has no inbox on this task")?;
        Ok(format!("{owner}:{}", row.key))
    }

    pub fn mark_effect_applied(&mut self, recipient: &Recipient, effect: u64) -> Result<()> {
        let owner = self.inbox_owner_now(recipient)?;
        let key = format!("{owner}/{effect:020}");
        let applied = self.rows_now(
            &RowQuery::key(TaskPart::InboxEffect, key.clone()),
            Counter::NestedRowDecodes,
        )?;
        if applied.is_empty() {
            self.put(plain(TaskPart::InboxEffect, key, effect.into()));
        }
        Ok(())
    }

    /// Replaces the scoped cursor of `recipient` for the cursor's adapter and binding.
    pub fn set_observe_cursor(
        &mut self,
        recipient: &Recipient,
        cursor: SessionCursor,
    ) -> Result<()> {
        let owner = self.inbox_owner_now(recipient)?;
        let token = cursor_token(&cursor.adapter, &serde_json::to_value(&cursor.binding)?)?;
        let prefix = format!("{owner}/");
        let existing = self
            .rows_now(
                &RowQuery::under(TaskPart::ObserveCursor, prefix.clone()).with_index(token.clone()),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .next();
        let doc = serde_json::to_value(&cursor)?;
        let key = match existing {
            Some(row) if row.doc == doc => return Ok(()),
            Some(row) => {
                self.history_edit = true;
                row.key
            }
            None => format!(
                "{prefix}{}",
                seq(self.last_key(TaskPart::ObserveCursor, &prefix)? + 1)
            ),
        };
        let mut row = plain(TaskPart::ObserveCursor, key, doc);
        row.index = vec![token];
        self.put(row);
        Ok(())
    }

    /// Replaces the usage of the run (no child) or of one of its review children, entries
    /// included.
    pub fn set_usage(&mut self, run: &str, child: Option<&str>, usage: Usage) -> Result<()> {
        match child {
            Some(child) => self.edit_review_child(run, child, |c| {
                c.usage = Some(usage);
                Ok(())
            }),
            None => {
                let s = self.current_run_seq(run)?;
                let prefix = format!("{}/", child_key(s, SELF));
                for row in self.rows_now(
                    &RowQuery::under(TaskPart::RunUsage, prefix.clone()),
                    Counter::NestedRowDecodes,
                )? {
                    self.delete(row.part, row.key);
                }
                for (i, entry) in usage.entries.iter().enumerate() {
                    self.put(plain(
                        TaskPart::RunUsage,
                        format!("{prefix}{}", seq(i + 1)),
                        serde_json::to_value(entry)?,
                    ));
                }
                let current = self.head.current_run.as_mut().context("no current run")?;
                current.usage = Some(UsageTotals::of(usage));
                self.usage_dirty = true;
                Ok(())
            }
        }
    }

    /// Appends response entries to one owner and recomputes its usage from all of them.
    pub fn append_usage_entries(
        &mut self,
        run: &str,
        child: Option<&str>,
        entries: Vec<UsageEntry>,
    ) -> Result<()> {
        let s = self.current_run_seq(run)?;
        let owner = match child {
            Some(child) => {
                return self.edit_review_child(run, child, |c| {
                    let mut all = c.usage.take().map(|u| u.entries).unwrap_or_default();
                    all.extend(entries);
                    c.usage = Some(Usage::from_entries(all));
                    Ok(())
                });
            }
            None => child_key(s, SELF),
        };
        let mut all: Vec<UsageEntry> = self
            .rows_now(
                &RowQuery::under(TaskPart::RunUsage, format!("{owner}/")),
                Counter::NestedRowDecodes,
            )?
            .into_iter()
            .map(|row| Ok(serde_json::from_value(row.doc)?))
            .collect::<Result<_>>()?;
        all.extend(entries);
        self.set_usage(run, None, Usage::from_entries(all))
    }

    /// Recomputes the current run's usage facts (and its current child's) from the run's own
    /// usage records, as `Run::total_usage` and the card's cost rule define them.
    pub(crate) fn recompute_usage(&mut self) -> Result<()> {
        let Some(s) = self.head.seqs.run else {
            return Ok(());
        };
        let run = self.head.current_run.clone().context("no current run")?;
        let mut rows = RowMap::new();
        let mut put = |row: HistoryRow| {
            rows.insert((row.part, row.key.clone()), row);
        };
        put(super::head_mut::run_row(s, SELF, &run, true)?);
        for row in self.rows_now(
            &RowQuery::under(TaskPart::ReviewRun, format!("{}/", seq(s))),
            Counter::NestedRowDecodes,
        )? {
            put(row);
        }
        if let (Some(child), Some(c)) = (run.current_child.as_deref(), self.head.seqs.run_child) {
            put(super::head_mut::run_row(s, c, child, true)?);
        }
        for row in self.rows_now(
            &RowQuery::under(TaskPart::RunUsage, format!("{}/", seq(s))),
            Counter::NestedRowDecodes,
        )? {
            put(row);
        }
        let view = TaskRows::new(rows.values());
        let own = super::head_mut::run_row(s, SELF, &run, true)?;
        let assembled: Run = serde_json::from_value(run_value(&view, s, SELF, &own.doc)?)?;
        let current = self.head.current_run.as_mut().context("no current run")?;
        current.facts.usage_total = assembled.total_usage().map(UsageTotals::of);
        current.facts.cost = CostTotals::of_run(&assembled);
        if let (Some(child), Some(full)) = (
            current.current_child.as_deref_mut(),
            assembled.review_runs.last(),
        ) {
            child.facts.usage_total = full.total_usage().map(UsageTotals::of);
            child.facts.cost = CostTotals::of_run(full);
        }
        *self.seen = self.head.clone();
        Ok(())
    }
}

pub(crate) fn plain(part: TaskPart, key: String, doc: Value) -> HistoryRow {
    HistoryRow {
        part,
        key,
        current: false,
        open: false,
        doc,
        index: Vec::new(),
    }
}
