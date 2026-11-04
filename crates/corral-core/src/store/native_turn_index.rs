//! Fixed-width sequence ordering and generation maximum pointers for native turns.
use crate::agent::AgentTurn;
use crate::backend::{HistoryRow, TaskPart};
use anyhow::{Context, Result};
use serde_json::{Value, json};

fn row(key: String, doc: Value) -> HistoryRow {
    HistoryRow {
        part: TaskPart::AgentRecord,
        key,
        current: false,
        open: false,
        doc,
        index: Vec::new(),
    }
}

/// Sequence-ordered deletion metadata; the canonical turn row stays in append order.
pub(super) fn turn_order_row(owner: &str, turn: &AgentTurn, key: &str) -> HistoryRow {
    row(
        format!(
            "{owner}/turn_order/{:010}/{:020}",
            turn.generation, turn.seq
        ),
        json!({"key": key}),
    )
}

pub(super) fn latest_turn_seq(doc: &Value) -> Result<u64> {
    doc["seq"]
        .as_str()
        .context("native latest-turn pointer has no sequence")?
        .parse()
        .context("native latest-turn pointer sequence is malformed")
}

/// Derived max-sequence pointer. Turn storage order remains append order for full facades.
pub(super) fn latest_turn_row(owner: &str, generation: u32, turn: u64) -> HistoryRow {
    row(
        format!("{owner}/latest_turn/{generation:010}"),
        json!({"seq": format!("{turn:020}")}),
    )
}
