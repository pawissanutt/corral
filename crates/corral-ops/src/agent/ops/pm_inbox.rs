//! The Pi-native PM's PM inbox operations (T-544 pm-inbox): `inbox_read` pages open items or
//! reads one, and `inbox_resolve` claims or resolves one at its current revision through the
//! same store transitions as `corral pm inbox`. Items are workspace-wide, not project-bound.
use anyhow::Result;
use corral_core::agent::RESULT_MAX_BYTES;
use corral_core::pm_inbox::{ItemState, PmInboxItem, PmInboxRefusal, Resolution, name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::call::{RejectCode, Rejection};
use crate::Ctx;

const CHUNK_MAX: usize = 2048;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Read {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    state: Option<ItemState>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Claim,
    Resolve,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolve {
    pub(super) id: String,
    expected_revision: u32,
    action: Action,
    #[serde(default)]
    outcome: Option<Resolution>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    refs: Vec<String>,
}

pub(super) const READ_FIELDS: &str = "id?, state?, offset?, limit?";
pub(super) const RESOLVE_FIELDS: &str = "id, expected_revision, action, outcome?, text?, refs?";

/// A store refusal as the agent rejection the receipt records; a stale revision keeps its code.
fn rejection(error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<PmInboxRefusal>() {
        Some(refusal) if refusal.code == "stale_revision" => {
            Rejection::new(RejectCode::StaleRevision, refusal.detail.clone()).into()
        }
        Some(refusal) => Rejection::new(RejectCode::BadRequest, refusal.to_string()).into(),
        None => error,
    }
}

fn summary(item: &PmInboxItem) -> Value {
    json!({"id": item.id, "revision": item.revision, "state": item.state, "from": item.from,
        "kind": item.kind, "subject": item.subject, "created": item.created, "refs": item.refs,
        "requested_action": item.requested_action.as_ref().map(|a| a.command())})
}

pub(super) fn read(ctx: &Ctx, p: Read) -> Result<Value> {
    let Some(id) = p.id else {
        let items = ctx
            .store
            .pm_inbox_items()?
            .iter()
            .filter(|item| p.state.map_or(item.pending(), |state| item.state == state))
            .map(summary)
            .collect();
        return super::pm::page(items, p.offset, p.limit.unwrap_or(20));
    };
    let item = ctx.store.pm_inbox_item(&id).map_err(rejection)?;
    let limit = p.limit.unwrap_or(CHUNK_MAX);
    if limit == 0 || limit > CHUNK_MAX {
        return Err(Rejection::new(
            RejectCode::BadRequest,
            format!("limit must be 1..{CHUNK_MAX} characters"),
        )
        .into());
    }
    // The owner quote and body can exceed one frame; page them as one JSON text by characters.
    let text = serde_json::to_string(&json!({
        "owner_quote": item.owner_words.as_ref().map(|w| &w.quote), "body": item.body,
    }))?;
    let total = text.chars().count();
    let chunk: String = text.chars().skip(p.offset).take(limit).collect();
    let next = p.offset.saturating_add(chunk.chars().count());
    let mut value = json!({"item": summary(&item), "claimed": item.claimed,
        "withdrawn": item.withdrawn, "content_json": chunk, "total_chars": total,
        "next_offset": (next < total).then_some(next)});
    if serde_json::to_vec(&value)?.len() + 256 < RESULT_MAX_BYTES {
        value["outcome"] = json!(item.outcome);
    } else {
        value["outcome"] = json!(item.outcome.as_ref().map(|o| name(&o.resolution)));
    }
    Ok(value)
}

pub(super) fn resolve(ctx: &Ctx, p: &Resolve, request: &str) -> Result<Value> {
    let item = match p.action {
        Action::Claim => ctx
            .store
            .pm_inbox_claim(&p.id, p.expected_revision, Some(request)),
        Action::Resolve => {
            let outcome = p.outcome.ok_or_else(|| {
                Rejection::new(
                    RejectCode::BadRequest,
                    "resolve needs an outcome: done, declined or escalated_to_owner",
                )
            })?;
            crate::pm_inbox::resolve(
                ctx,
                &p.id,
                p.expected_revision,
                (
                    outcome,
                    p.text.as_deref().unwrap_or_default(),
                    p.refs.clone(),
                ),
                Some(request),
            )
        }
    }
    .map_err(rejection)?;
    Ok(json!({"outcome": name(&item.state), "item": item.id, "revision": item.revision}))
}
