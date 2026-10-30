//! Native lifecycle writes touch one indexed child and its bounded container metadata.
use super::{TaskHead, TaskHeadMut};
use crate::agent::{ContainerRef, NativeRecord};
use crate::backend::{HistoryRow, RowQuery, TaskPart};
use crate::work::Counter;
use crate::{AgentHostHead, ContainerFacts, TaskRef};
use anyhow::{Context, Result, ensure};

fn current_host<'a>(
    head: &'a mut TaskHead,
    container: &ContainerRef,
) -> Option<(
    &'a mut Option<Box<AgentHostHead>>,
    Option<&'a mut ContainerFacts>,
)> {
    match container {
        ContainerRef::Worker { run, .. } => head
            .current_run
            .as_deref_mut()
            .filter(|r| r.id == *run)
            .map(|r| (&mut r.agent_host, Some(&mut r.facts))),
        ContainerRef::Reviewer { run, review, .. } => head
            .current_run
            .as_deref_mut()
            .filter(|r| r.id == *run)
            .and_then(|r| r.current_child.as_deref_mut())
            .filter(|r| r.id == *review)
            .map(|r| (&mut r.agent_host, Some(&mut r.facts))),
        ContainerRef::Planner { run, .. } => head
            .last_plan_run
            .as_deref_mut()
            .filter(|r| r.id == *run)
            .map(|r| (&mut r.agent_host, Some(&mut r.facts))),
        ContainerRef::PlanReviewer { plan, review, .. } => head
            .last_plan
            .as_deref_mut()
            .filter(|p| p.id == *plan)
            .and_then(|p| p.current_review.as_mut())
            .filter(|r| r.id == *review)
            .map(|r| (&mut r.agent_host, None)),
        ContainerRef::Pm => None,
    }
}
fn coordinates(container: &ContainerRef, owner: &str) -> Result<(TaskPart, String)> {
    Ok(match container {
        ContainerRef::Worker { .. } => (
            TaskPart::Run,
            owner
                .split('/')
                .next()
                .context("native run owner missing")?
                .into(),
        ),
        ContainerRef::Reviewer { .. } => (TaskPart::ReviewRun, owner.into()),
        ContainerRef::Planner { .. } => (
            TaskPart::PlanRun,
            owner
                .strip_prefix("plan_run:")
                .context("native planner owner malformed")?
                .into(),
        ),
        ContainerRef::PlanReviewer { .. } => (
            TaskPart::PlanReview,
            owner
                .strip_prefix("plan_review:")
                .context("native plan-review owner malformed")?
                .into(),
        ),
        ContainerRef::Pm => anyhow::bail!("PM state does not belong to a task"),
    })
}
fn facts(record: &NativeRecord) -> (u64, u64) {
    match record {
        NativeRecord::Question(q) => (u64::from(q.state.pending()), 0),
        NativeRecord::Wake(_) => (0, u64::from(record.metadata().0)),
        _ => (0, 0),
    }
}

impl TaskHeadMut {
    /// Edit one existing PM receipt without loading its task's receipt history.
    pub fn edit_pm_receipt(
        &mut self,
        request_id: &str,
        edit: impl FnOnce(&mut crate::agent::RequestReceipt) -> Result<()>,
    ) -> Result<bool> {
        let Some(mut row) = self
            .overlaid()?
            .query(
                &RowQuery::part(TaskPart::PmReceipt).with_index(format!("request:{request_id}")),
                1,
                Counter::HistoryRowDecodes,
            )?
            .pop()
        else {
            return Ok(false);
        };
        let mut receipt: crate::agent::RequestReceipt =
            serde_json::from_value(row.doc.clone()).context("decoding PM receipt")?;
        edit(&mut receipt)?;
        ensure!(
            receipt.request_id == request_id,
            "PM receipt identity cannot change"
        );
        let doc = serde_json::to_value(receipt).context("encoding PM receipt")?;
        if doc == row.doc {
            return Ok(false);
        }
        row.doc = doc;
        self.history_edit = true;
        self.put(row);
        Ok(true)
    }

    fn native_row(&self, part: TaskPart, key: &str) -> Result<HistoryRow> {
        let mut q = RowQuery::part(part);
        q.key = Some(key.into());
        self.rows_now(&q, Counter::NestedRowDecodes)?
            .pop()
            .context("native container row disappeared")
    }

    fn edit_native(
        &mut self,
        container: &ContainerRef,
        edit: impl FnOnce(&mut Option<Box<AgentHostHead>>, Option<&mut ContainerFacts>) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            container.task() == Some(self.head.id.as_str()),
            "native container belongs to another task"
        );
        self.adjust_projections()?;
        if let Some((host, facts)) = current_host(&mut self.head, container) {
            return edit(host, facts);
        }
        let owner = self
            .overlaid()?
            .agent_owner(container)?
            .context("native container missing")?;
        let (part, key) = coordinates(container, &owner)?;
        let mut row = self.native_row(part, &key)?;
        let before = row.doc.clone();
        let mut host: Option<Box<AgentHostHead>> = serde_json::from_value(
            row.doc
                .get("agent_host")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )?;
        let mut facts: Option<ContainerFacts> = row
            .doc
            .get(super::codec::FACTS)
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        edit(&mut host, facts.as_mut())?;
        row.doc["agent_host"] = serde_json::to_value(host)?;
        if let Some(facts) = facts {
            row.doc[super::codec::FACTS] = serde_json::to_value(facts)?;
        }
        if row.doc != before {
            self.history_edit = true;
            self.put(row);
            self.bump_native_parent(part, &key)?;
        }
        Ok(())
    }

    fn bump_native_parent(&mut self, child: TaskPart, key: &str) -> Result<()> {
        let parent = match child {
            TaskPart::ReviewRun => TaskPart::Run,
            TaskPart::PlanReview => TaskPart::Plan,
            _ => return Ok(()),
        };
        let key = key
            .split('/')
            .next()
            .context("native child owner missing")?;
        let seq: usize = key.parse().context("native parent ordinal malformed")?;
        if parent == TaskPart::Run && self.head.seqs.run == Some(seq) {
            self.head
                .current_run
                .as_deref_mut()
                .context("native parent run missing")?
                .facts
                .children_rev += 1;
        } else if parent == TaskPart::Plan && self.head.seqs.plan == Some(seq) {
            self.head
                .last_plan
                .as_deref_mut()
                .context("native parent plan missing")?
                .facts
                .children_rev += 1;
        } else {
            let mut row = self.native_row(parent, key)?;
            let mut facts: ContainerFacts =
                serde_json::from_value(row.doc[super::codec::FACTS].clone())?;
            facts.children_rev += 1;
            row.doc[super::codec::FACTS] = serde_json::to_value(facts)?;
            self.put(row);
        }
        Ok(())
    }

    /// Initialize only the scalar host; records are inserted separately in the same unit of work.
    pub fn init_agent_head(&mut self, container: &ContainerRef, host: AgentHostHead) -> Result<()> {
        ensure!(
            host.records == Default::default(),
            "a new native head cannot claim existing records"
        );
        self.edit_native(container, |slot, _| {
            ensure!(slot.is_none(), "native host already initialized");
            *slot = Some(Box::new(host));
            Ok(())
        })
    }

    /// Edits binding/controls, never append positions or child histories.
    pub fn edit_agent_head(
        &mut self,
        container: &ContainerRef,
        edit: impl FnOnce(&mut AgentHostHead) -> Result<()>,
    ) -> Result<()> {
        self.edit_native(container, |slot, _| {
            let head = slot.as_deref_mut().context("native host missing")?;
            let counts = head.records.clone();
            edit(head)?;
            ensure!(
                head.records == counts,
                "native append positions are store-owned"
            );
            Ok(())
        })
    }

    /// Upserts one identified record. Appends use fixed-size positions, not a last-row/history read.
    pub fn put_agent_record(
        &mut self,
        container: &ContainerRef,
        record: NativeRecord,
    ) -> Result<bool> {
        let view = self.overlaid()?;
        let owner = view
            .agent_owner(container)?
            .context("native container missing")?;
        let before = view
            .agent_rows(container, record.kind(), Some(record.token()), false)?
            .pop();
        let doc = serde_json::to_value(&record)?;
        if before.as_ref().is_some_and(|row| row.doc == doc) {
            return Ok(false);
        }
        let old = before
            .as_ref()
            .map(|row| NativeRecord::from_doc(record.kind(), row.doc.clone()))
            .transpose()?;
        let append = before.is_none();
        let head = view.agent_head(container)?.context("native host missing")?;
        let kind = super::codec::AGENT_KINDS
            .iter()
            .position(|kind| *kind == record.kind())
            .context("native record family missing")?;
        let ordinal = head.records.counts[kind]
            .checked_add(1)
            .context("native record ordinal overflow")?;
        let key = before
            .as_ref()
            .map(|row| row.key.clone())
            .unwrap_or_else(|| format!("{owner}/{}/{ordinal:06}", record.kind()));
        let (open, index) = record.metadata();
        let old_facts = old.as_ref().map(facts).unwrap_or_default();
        let new_facts = facts(&record);
        self.edit_native(container, |slot, facts| {
            let head = slot.as_deref_mut().context("native host missing")?;
            if append {
                head.records.counts[kind] = ordinal;
            }
            head.records.pending = head
                .records
                .pending
                .saturating_sub(u64::from(old.as_ref().is_some_and(NativeRecord::pending)))
                + u64::from(record.pending());
            if let Some(facts) = facts {
                facts.agent_records += u64::from(append);
                facts.pending_questions =
                    facts.pending_questions.saturating_sub(old_facts.0) + new_facts.0;
                facts.armed_wakes = facts.armed_wakes.saturating_sub(old_facts.1) + new_facts.1;
            }
            Ok(())
        })?;
        if !append {
            self.history_edit = true;
        }
        self.put(HistoryRow {
            part: TaskPart::AgentRecord,
            key,
            current: false,
            open,
            doc,
            index,
        });
        if let NativeRecord::Turn(turn) = &record {
            let pointer = super::codec::latest_turn_row(&owner, turn.generation, turn.seq);
            let mut q = RowQuery::part(TaskPart::AgentRecord);
            q.key = Some(pointer.key.clone());
            let previous = self.rows_now(&q, Counter::NestedRowDecodes)?.pop();
            let previous_seq = previous
                .as_ref()
                .map(|row| super::codec::latest_turn_seq(&row.doc))
                .transpose()?;
            if previous_seq.is_none_or(|seq| seq < turn.seq) {
                self.history_edit |= previous.is_some();
                self.put(pointer);
            }
        }
        Ok(true)
    }
}
