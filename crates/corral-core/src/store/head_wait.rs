//! Keyed wait edits on a named coding run, including terminal/replaced workers with owed jobs.
use super::codec::{SELF, child_key, seq};
use super::head::{last_ordinal, projection};
use super::head_mut::{TaskHeadMut, run_row};
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use crate::{ContainerFacts, RunHead, WaitRecord, WaitState};
use anyhow::{Context, Result, ensure};

pub(super) fn adjust_facts(
    task: &str,
    facts: &mut ContainerFacts,
    before: &WaitRecord,
    after: &WaitRecord,
) {
    let old = ContainerFacts::wait_parking(task, before);
    let new = ContainerFacts::wait_parking(task, after);
    facts.dependency_waits = facts
        .dependency_waits
        .saturating_sub(old.0)
        .saturating_add(new.0);
    facts.nondependency_waits = facts
        .nondependency_waits
        .saturating_sub(old.1)
        .saturating_add(new.1);
    let pending = |wait: &WaitRecord| i64::from(wait.observation.state == WaitState::Pending);
    let open = |wait: &WaitRecord| i64::from(wait.receipt.is_none());
    facts.pending_waits = facts
        .pending_waits
        .saturating_add_signed(pending(after) - pending(before));
    facts.open_obligations = facts
        .open_obligations
        .saturating_add_signed(open(after) - open(before));
    facts.settled_job_exits = facts.settled_job_exits.saturating_add_signed(
        i64::from(after.settled_job_exit()) - i64::from(before.settled_job_exit()),
    );
    facts.obligations_rev += 1;
}

impl TaskHeadMut {
    /// Edits one named run's own wait. Unlike edit_wait this explicitly permits historical runs.
    pub fn edit_run_wait(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut WaitRecord) -> Result<()>,
    ) -> Result<()> {
        self.adjust_projections()?;
        if self
            .head
            .current_run
            .as_ref()
            .is_some_and(|current| current.id == run)
        {
            return self.edit_wait(run, id, edit);
        }
        let s = self
            .overlaid()?
            .run_seq(run)?
            .with_context(|| format!("run {run} not found"))?;
        let q = RowQuery::under(
            TaskPart::RunObligation,
            format!("{}/wait/", child_key(s, SELF)),
        )
        .with_index(format!("id:{id}"));
        let row = self
            .overlaid()?
            .query(&q, 1, Counter::NestedRowDecodes)?
            .pop()
            .with_context(|| format!("wait {id} not found"))?;
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
            after.receipt.as_ref().map(|receipt| receipt.id.as_str()),
            after.open(run),
        );
        if new == row {
            return Ok(());
        }
        let parent = self
            .overlaid()?
            .query(
                &RowQuery::key(TaskPart::Run, seq(s)),
                1,
                Counter::HistoryRowDecodes,
            )?
            .pop()
            .context("wait parent run missing")?;
        let (mut before_run, facts): (RunHead, ContainerFacts) = projection(&parent.doc)?;
        before_run.facts = facts;
        let mut after_run = before_run.clone();
        adjust_facts(&self.head.id, &mut after_run.facts, &before, &after);
        self.head.summary.adjust_run_head(&before_run, &after_run);
        self.history_edit = true;
        self.put(new);
        self.put(run_row(s, SELF, &after_run, false)?);
        Ok(())
    }
}
