//! Event inputs and oldest-ended retention never materialize a native history vector.
#[cfg(test)]
#[path = "head_turn_retention_tests.rs"]
mod tests;
use super::{TaskHead, TaskHeadMut};
use crate::TaskRef;
use crate::agent::{AgentRunState, AgentTurn, ContainerRef, NativeRecord, TURNS_MAX};
use crate::backend::{RowQuery, TaskPart};
use crate::work::Counter;
use anyhow::{Context, Result};

impl TaskHead {
    /// A lifecycle callback's selected turn, generation and classification witnesses only.
    /// This is not a full-state reader and must not be used as a replacement snapshot.
    pub fn agent_event_inputs(
        &self,
        container: &ContainerRef,
        generation: u32,
        seq: u64,
    ) -> Result<AgentRunState> {
        let host = self.agent_head(container)?.context("native host missing")?;
        let mut state: AgentRunState = serde_json::from_value(serde_json::to_value(host)?)?;
        state
            .turns
            .extend(self.agent_turn(container, generation, seq)?);
        state
            .generations
            .extend(self.agent_generation(container, generation)?);
        let report = if state.binding.role == crate::agent::AgentRole::Reviewer {
            "verdict"
        } else {
            "done"
        };
        for (kind, token, open) in [
            (
                "receipts",
                Some(format!("report:{report}:{generation}:{seq}")),
                false,
            ),
            ("questions", None, true),
            ("wakes", None, true),
        ] {
            let Some(owner) = self.agent_owner(container)? else {
                continue;
            };
            let mut query = RowQuery::under(TaskPart::AgentRecord, format!("{owner}/{kind}/"));
            query.index = token;
            query.open_only = open;
            if let Some(row) = self.query(&query, 1, Counter::NestedRowDecodes)?.pop() {
                NativeRecord::from_doc(kind, row.doc)?.put(&mut state);
            }
        }
        Ok(state)
    }
}

impl TaskHeadMut {
    /// Callback inputs including earlier writes in this transaction.
    pub fn agent_event_inputs(
        &self,
        container: &ContainerRef,
        generation: u32,
        seq: u64,
    ) -> Result<AgentRunState> {
        self.overlaid()?
            .agent_event_inputs(container, generation, seq)
    }

    /// Retains append order, evicting only ended turns, even when open turns exceed the cap.
    /// Each eviction repairs the per-generation maximum with one ordered metadata lookup.
    pub fn retain_agent_turns(&mut self, container: &ContainerRef) -> Result<()> {
        self.retain_agent_turns_to(container, TURNS_MAX as u64)
    }

    fn retain_agent_turns_to(&mut self, container: &ContainerRef, limit: u64) -> Result<()> {
        loop {
            let view = self.overlaid()?;
            let host = view.agent_head(container)?.context("native host missing")?;
            if host.records.live[3] <= limit {
                return Ok(());
            }
            let owner = view
                .agent_owner(container)?
                .context("native container missing")?;
            let query = RowQuery::under(TaskPart::AgentRecord, format!("{owner}/turns/"))
                .with_index("turn:ended");
            let Some(row) = view.query(&query, 1, Counter::NestedRowDecodes)?.pop() else {
                return Ok(());
            };
            let turn: AgentTurn = serde_json::from_value(row.doc)?;
            self.history_edit = true;
            self.delete(TaskPart::AgentRecord, row.key);
            let order = super::codec::turn_order_row(&owner, &turn, "");
            self.delete(TaskPart::AgentRecord, order.key);
            self.edit_native(container, |slot, facts| {
                let host = slot.as_deref_mut().context("native host missing")?;
                host.records.live[3] -= 1;
                if let Some(facts) = facts {
                    facts.agent_records -= 1;
                }
                Ok(())
            })?;
            let view = self.overlaid()?;
            let query = RowQuery::under(
                TaskPart::AgentRecord,
                format!("{owner}/turn_order/{:010}/", turn.generation),
            )
            .newest_first();
            let pointer = super::codec::latest_turn_row(&owner, turn.generation, turn.seq);
            match view.query(&query, 1, Counter::NestedRowDecodes)?.pop() {
                Some(row) => {
                    let seq = row
                        .key
                        .rsplit('/')
                        .next()
                        .context("turn order missing")?
                        .parse()
                        .context("turn order malformed")?;
                    self.put(super::codec::latest_turn_row(&owner, turn.generation, seq));
                }
                None => self.delete(TaskPart::AgentRecord, pointer.key),
            }
        }
    }
}
