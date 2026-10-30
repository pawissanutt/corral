//! Worker-only selectors keep other native roles out before decoding receipt/wake history.
use crate::backend::{HistoryRow, TaskPart};

pub(super) fn decorate(row: &mut HistoryRow) {
    row.index.retain(|token| {
        token != "worker:wake:active" && !token.starts_with("worker:queued:effect:")
    });
    if row.part != TaskPart::AgentRecord {
        return;
    }
    let mut parts = row.key.split('/');
    let (Some(run), Some(child), Some(kind), Some(_)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return;
    };
    if parts.next().is_some()
        || run.parse::<usize>().is_err()
        || child != super::codec::seq(super::codec::SELF)
    {
        return;
    }
    let tags: Vec<_> = row
        .index
        .iter()
        .filter(|token| {
            (kind == "wakes" && token.as_str() == "wake:active")
                || (kind == "receipts" && token.starts_with("queued:effect:"))
        })
        .map(|token| format!("worker:{token}"))
        .collect();
    row.index.extend(tags);
}
