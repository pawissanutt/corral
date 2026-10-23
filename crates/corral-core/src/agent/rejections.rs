//! One `agent_request_rejected` event line per distinct refused request. A refusal never
//! writes a container, so identical retries are deduplicated in the event log itself, under a
//! store lock that makes concurrent retries record once as well.
use anyhow::Result;
use serde_json::{Value, json};

use crate::Store;

pub const REJECTED_EVENT: &str = "agent_request_rejected";

/// Appends the event unless one with the same `key` exists; true when it appended.
pub fn log_rejection_once(
    store: &Store,
    task: Option<&str>,
    key: &str,
    mut detail: Value,
) -> Result<bool> {
    let _lock = store.lock("agent-rejections")?;
    let seen = store.events_shared()?.iter().any(|event| {
        event.kind == REJECTED_EVENT && event.detail.get("key").and_then(Value::as_str) == Some(key)
    });
    if seen {
        return Ok(false);
    }
    detail["key"] = json!(key);
    store.log_event(task, REJECTED_EVENT, detail)?;
    Ok(true)
}
