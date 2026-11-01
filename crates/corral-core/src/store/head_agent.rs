//! The current worker's native-agent records, independently of its obligations and checkpoints.
use anyhow::{Context, Result};

use super::codec::{AGENT_KINDS, Rows, SELF, child_key, split_run};
use super::head::TaskHead;
use super::head_mut::TaskHeadMut;
use crate::agent::{AgentRunState, ContainerRef, NativeRecord};
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;

impl TaskHead {
    /// Reads only the current worker's agent-record family, keyed by its run ordinal.
    pub fn current_agent_state(&self) -> Result<Option<Box<AgentRunState>>> {
        let Some(run) = self.current_run.as_ref() else {
            return Ok(None);
        };
        let Some(host) = run.agent_host.as_ref() else {
            return Ok(None);
        };
        let s = self.seqs.run.context("current run ordinal missing")?;
        let mut doc = serde_json::to_value(host)?;
        for kind in AGENT_KINDS {
            let rows = self.query(
                &RowQuery::under(
                    TaskPart::AgentRecord,
                    format!("{}/{kind}/", child_key(s, SELF)),
                ),
                u32::MAX,
                Counter::NestedRowDecodes,
            )?;
            let records: Vec<_> = rows.into_iter().map(|row| row.doc).collect();
            if kind == "budget_requests" {
                // The physical family is flat, but the cumulative public ledger is nested.
                doc["budget"] = serde_json::json!({"requests": records});
            } else {
                doc[kind] = records.into();
            }
        }
        Ok(Some(Box::new(serde_json::from_value(doc)?)))
    }
}

impl TaskHeadMut {
    /// Edits one worker request receipt, including receipts of earlier runs.
    pub fn edit_worker_receipt(
        &mut self,
        run: &str,
        request: &str,
        edit: impl FnOnce(&mut crate::agent::RequestReceipt) -> Result<()>,
    ) -> Result<()> {
        let owner = self
            .head
            .owner_key(run, None)?
            .context("receipt run missing")?;
        let q = RowQuery::under(TaskPart::AgentRecord, format!("{owner}/receipts/"))
            .with_index(format!("request:{request}"));
        let row = self
            .rows_now(&q, Counter::NestedRowDecodes)?
            .into_iter()
            .next()
            .context("receipt missing")?;
        let mut receipt = serde_json::from_value(row.doc.clone())?;
        edit(&mut receipt)?;
        self.write_agent_record(
            &ContainerRef::Worker {
                task: self.head.id.clone(),
                run: run.into(),
            },
            NativeRecord::Receipt(receipt),
            Some(row),
        )
        .map(drop)
    }

    /// Replaces the current worker's native-agent records, leaving every other run family intact.
    pub fn set_current_agent_state(&mut self, state: Box<AgentRunState>) -> Result<()> {
        self.adjust_projections()?;
        let s = self.head.seqs.run.context("current run ordinal missing")?;
        let mut run = self
            .head
            .to_shell_task()?
            .runs
            .pop()
            .context("missing_run")?;
        let mut old = std::collections::BTreeMap::new();
        for row in self.rows_now(
            &RowQuery::under(TaskPart::AgentRecord, format!("{}/", child_key(s, SELF))),
            Counter::NestedRowDecodes,
        )? {
            old.insert((row.part, row.key.clone()), row);
        }
        run.agent_host = self.overlaid()?.current_agent_state()?;
        let before = crate::ContainerFacts::of_run(&self.head.id, "agent", &run)?;
        run.agent_host = Some(state);
        let after = crate::ContainerFacts::of_run(&self.head.id, "agent", &run)?;
        let facts = &mut self.head.current_run.as_mut().context("missing_run")?.facts;
        facts.agent_records = facts.agent_records - before.agent_records + after.agent_records;
        facts.pending_questions =
            facts.pending_questions - before.pending_questions + after.pending_questions;
        facts.armed_wakes = facts.armed_wakes - before.armed_wakes + after.armed_wakes;
        let mut rows = Rows::new();
        split_run(
            &self.head.id,
            &mut rows,
            s,
            SELF,
            &run,
            serde_json::to_value(&run)?,
            true,
        )?;
        let mut new = rows
            .0
            .into_iter()
            .filter(|((part, _), _)| *part == TaskPart::AgentRecord)
            .collect();
        super::write::carry_revisions(&old, &mut new);
        self.write_diff(&old, &new)?;
        self.head
            .current_run
            .as_mut()
            .context("missing_run")?
            .agent_host = run
            .agent_host
            .as_deref()
            .map(crate::AgentHostHead::of)
            .map(Box::new);
        Ok(())
    }
}
