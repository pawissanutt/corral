//! Task-wide worker wake/effect selectors: exact indexed reads, not container/history scans.
use super::TaskHead;
use crate::agent::{ContainerRef, RequestReceipt, WakeSubscription};
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Context, Result};
use std::collections::BTreeMap;

impl TaskHead {
    /// Armed/pending worker wakes, including historical/settled containers, in append order.
    pub fn active_worker_wakes(&self) -> Result<Vec<(ContainerRef, WakeSubscription)>> {
        let mut parents = BTreeMap::new();
        let mut out = Vec::new();
        for row in self.query(
            &RowQuery::part(TaskPart::AgentRecord)
                .with_index("worker:wake:active")
                .open(),
            u32::MAX,
            Counter::NestedRowDecodes,
        )? {
            let (parent, _) = row
                .key
                .split_once('/')
                .context("worker wake coordinate malformed")?;
            let run = self.native_parent_id(TaskPart::Run, parent, &mut parents)?;
            out.push((
                ContainerRef::Worker {
                    task: self.id.clone(),
                    run,
                },
                serde_json::from_value(row.doc).context("decoding active worker wake")?,
            ));
        }
        Ok(out)
    }

    /// First append-order queued effect receipt on an implementation worker (not its children).
    pub fn worker_queued_effect_receipt(&self, reference: &str) -> Result<Option<RequestReceipt>> {
        self.query(
            &RowQuery::part(TaskPart::AgentRecord)
                .with_index(format!("worker:queued:effect:{reference}")),
            1,
            Counter::NestedRowDecodes,
        )?
        .pop()
        .map(|row| serde_json::from_value(row.doc).context("decoding queued worker effect receipt"))
        .transpose()
    }
}
