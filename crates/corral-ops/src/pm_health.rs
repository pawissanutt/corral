//! Read-only stall signals; retained version-skew events are historical, not liveness probes.
use anyhow::Result;
use corral_core::backend::{HeadQuery, RowQuery, TaskPart};
use serde_json::{Value, json};

use crate::Ctx;

pub fn pm_health(ctx: &Ctx) -> Result<Value> {
    let mut unfinished = Vec::new();
    // This historical owner readback intentionally includes Done tasks (FU-2).
    // Keep its historical diagnostics, without hydrating unrelated task families.
    let mut heads = ctx.store.heads(&HeadQuery::RecentByStatus {
        status: "done".into(),
    })?;
    heads.sort_by_key(|head| head.num);
    for head in heads {
        let runs =
            crate::pm_next::heads::rows(ctx, &head, &RowQuery::part(TaskPart::Run).newest_first())?;
        let children = crate::pm_next::heads::rows(
            ctx,
            &head,
            &RowQuery::part(TaskPart::ReviewRun).newest_first(),
        )?;
        for run in runs {
            for row in std::iter::once(&run).chain(
                children
                    .iter()
                    .filter(|child| child.key.starts_with(&format!("{}/", run.key))),
            ) {
                if row.doc["ended"].is_null() || row.doc["outcome"].is_null() {
                    unfinished.push(json!({"task":head.id,"run":row.doc["id"]}));
                }
            }
        }
    }
    Ok(json!({
        "sync_owned":crate::session_scope::owns_inventory(ctx)?,
        "selected_socket":ctx.herdr.socket_path()?,
        "done_unfinished_runs":unfinished,
        "last_version_skew":ctx.store.events_of(None, &["version_skew"])?.last(),
        "policy_delivery":crate::pm_policy::pm_policy_health(ctx)?,
        "supervisor":crate::pm_supervisor::health(ctx)?,
    }))
}
