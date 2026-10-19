//! Read-only stall signals; retained version-skew events are historical, not liveness probes.
use anyhow::Result;
use corral_core::Status;
use serde_json::{Value, json};

use crate::Ctx;

pub fn pm_health(ctx: &Ctx) -> Result<Value> {
    let mut unfinished = Vec::new();
    for task in ctx.store.list()? {
        if task.status != Status::Done {
            continue;
        }
        let mut pending: Vec<_> = task.runs.iter().collect();
        while let Some(run) = pending.pop() {
            pending.extend(&run.review_runs);
            if run.ended.is_none() || run.outcome.is_none() {
                unfinished.push(json!({"task":task.id,"run":run.id}));
            }
        }
    }
    Ok(json!({
        "sync_owned":crate::session_scope::owns_inventory(ctx)?,
        "selected_socket":ctx.herdr.socket_path()?,
        "done_unfinished_runs":unfinished,
        "last_version_skew":ctx.store.events_shared()?.iter().rev().find(|e| e.kind == "version_skew"),
        "policy_delivery":crate::pm_policy::pm_policy_health(ctx)?,
    }))
}
