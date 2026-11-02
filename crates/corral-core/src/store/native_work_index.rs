//! Derived native work tokens follow the canonical container/receipt row atomically.
use crate::backend::{HistoryRow, TaskPart};
use serde_json::Value;

pub(super) fn decorate_native(row: &mut HistoryRow) {
    if row.part == TaskPart::Plan {
        row.index
            .retain(|token| !token.starts_with("native:submitted_run:"));
        if let Some(run) = row.doc.get("run").and_then(Value::as_str) {
            row.index.push(format!("native:submitted_run:{run}"));
        }
        return;
    }
    if row.part == TaskPart::PmReceipt {
        // Same canonical receipt debt as native containers, indexed before decode.
        // Unknown metadata remains discoverable so decoding reports corruption.
        row.index.retain(|token| token != "native:work");
        let settled = matches!(
            row.doc.get("state").and_then(Value::as_str),
            Some("committed" | "rejected" | "abandoned")
        );
        let owed = row
            .doc
            .get("post_pending")
            .is_some_and(|pending| pending.as_array().is_none_or(|pending| !pending.is_empty()));
        if !settled || owed {
            row.index.push("native:work".into());
        }
        return;
    }
    if row.part == TaskPart::AgentRecord {
        super::native_worker_index::decorate(row);
        return;
    }
    if !matches!(
        row.part,
        TaskPart::Run | TaskPart::ReviewRun | TaskPart::PlanRun | TaskPart::PlanReview
    ) {
        return;
    }
    let native = row
        .doc
        .get("agent_host")
        .is_some_and(|host| !host.is_null())
        || row
            .doc
            .get("harness")
            .and_then(Value::as_str)
            .is_some_and(crate::is_corral_pi);
    let unsettled = native && row.doc.pointer("/inbox/settled").is_none_or(Value::is_null);
    let pending = row
        .doc
        .get("agent_host")
        .filter(|host| !host.is_null())
        .is_some_and(|host| {
            // Canonical heads encode fixed-width counts. Unknown metadata is conservative:
            // keep the owner discoverable rather than treating it as settled work.
            host.pointer("/_records/pending")
                .and_then(|count| count.as_u64().or_else(|| count.as_str()?.parse().ok()))
                .is_none_or(|count| count > 0)
        });
    row.index
        .retain(|token| !matches!(token.as_str(), "native:unsettled" | "native:work"));
    if unsettled {
        row.index.push("native:unsettled".into());
    }
    if unsettled || pending {
        row.index.push("native:work".into());
    }
}
