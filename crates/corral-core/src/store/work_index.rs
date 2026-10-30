//! Derived task-work index. The head ledger and container facts remain authoritative;
//! this single payload-free row only selects owed attention or held-slot neighborhood inputs.
use crate::backend::{HeadQuery, HistoryRow, TaskPart};
use crate::{AttentionLedger, Store, Task, TaskHead};
use anyhow::Result;

pub(super) const KEY: &str = "task";
fn attention(ledger: &AttentionLedger) -> bool {
    !ledger.outbox.is_empty()
        || !ledger.notice_outbox.is_empty()
        || !ledger.source_notices.is_empty()
        || ledger
            .records
            .values()
            .any(|record| record.resolved_at.is_none())
}
fn row(ledger: &AttentionLedger, slots: bool) -> Option<HistoryRow> {
    let mut index = Vec::new();
    if attention(ledger) {
        index.push("attention:owed".into());
    }
    if slots {
        index.push("slots:held".into());
    }
    (!index.is_empty()).then(|| HistoryRow {
        part: TaskPart::WorkIndex,
        key: KEY.into(),
        current: true,
        open: true,
        doc: serde_json::json!({}),
        index,
    })
}

pub(super) fn full(task: &Task) -> Option<HistoryRow> {
    let slots = task.launch.as_ref().is_some_and(|launch| launch.reserved)
        || task.review_reservation.is_some()
        || task.runs.iter().any(|run| {
            run.gate.as_ref().is_some_and(crate::GateRecord::holds_slot)
                || run
                    .review
                    .as_ref()
                    .is_some_and(crate::ReviewRecord::holds_slot)
        })
        || task
            .plans
            .iter()
            .any(|plan| plan.reviews.iter().any(|review| review.verdict.is_none()));
    row(&task.attention, slots)
}

pub(super) fn head(task: &TaskHead) -> Option<HistoryRow> {
    let slots = task.launch.as_ref().is_some_and(|launch| launch.reserved)
        || task.review_reservation.is_some()
        || task.current_run.as_ref().is_some_and(|run| {
            run.gate.as_ref().is_some_and(crate::GateRecord::holds_slot)
                || run
                    .review
                    .as_ref()
                    .is_some_and(crate::ReviewRecord::holds_slot)
        })
        || task.summary.held_gates > 0
        || task.summary.held_reviews > 0
        || task.summary.pending_plan_reviews > 0
        || task
            .last_plan
            .as_ref()
            .is_some_and(|plan| plan.facts.pending_children > 0);
    row(&task.attention, slots)
}

impl Store {
    /// Supplemental attention-neighborhood inputs, including terminal owed sources and
    /// terminal tasks holding slots. Resolved retained records alone are not ongoing work.
    /// The index is maintained atomically by both full and head writes.
    pub fn attention_input_heads(&self) -> Result<Vec<TaskHead>> {
        let frozen = self.read_only_snapshot()?;
        let view = frozen.view()?;
        let mut owners = std::collections::BTreeSet::new();
        for token in ["attention:owed", "slots:held"] {
            owners.extend(view.token_owners(TaskPart::WorkIndex, token, u32::MAX)?);
        }
        frozen.heads(&HeadQuery::Ids(
            owners
                .into_iter()
                .map(|num| format!("T-{:03}", num.0))
                .collect(),
        ))
    }
}
