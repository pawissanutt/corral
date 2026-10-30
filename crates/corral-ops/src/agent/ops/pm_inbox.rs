//! The Pi-native PM's PM inbox operations (T-544 pm-inbox): `inbox_read` pages open items or
//! reads one, and `inbox_resolve` claims or resolves one at its current revision through the
//! same store transitions as `corral pm inbox`. Items are workspace-wide, not project-bound.
use anyhow::Result;
use corral_core::pm_inbox::{ItemState, PmInboxItem, PmInboxRefusal, Resolution, name};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::call::{RejectCode, Rejection};
use crate::Ctx;

const CHUNK_MAX: usize = 1024;

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

/// A summary's refs and requested command each show at most this many bytes, so one summary
/// and one content page always fit a result frame; `refs_total` counts every ref and the
/// by-ID read pages the complete values.
const SUMMARY_DETAIL_MAX: usize = 512;

fn summary(item: &PmInboxItem) -> Value {
    let mut refs = Vec::new();
    let mut bytes = 0;
    for reference in &item.refs {
        bytes += reference.len() + 3;
        if bytes > SUMMARY_DETAIL_MAX {
            break;
        }
        refs.push(reference);
    }
    let action = item.requested_action.as_ref().map(|action| {
        let mut command = action.command();
        if command.len() > SUMMARY_DETAIL_MAX {
            let mut end = SUMMARY_DETAIL_MAX;
            while !command.is_char_boundary(end) {
                end -= 1;
            }
            command.truncate(end);
            command.push_str("...");
        }
        command
    });
    json!({"id": item.id, "revision": item.revision, "state": item.state, "from": item.from,
        "kind": item.kind, "subject": item.subject, "created": item.created, "refs": refs,
        "refs_total": item.refs.len(), "requested_action": action})
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
    // Every variable-size field can exceed one frame together; page them as one JSON text by
    // characters. A page's character re-escapes to at most 4 bytes, so 1024 characters plus
    // the bounded summary stay under the 8 KiB result cap.
    let text = serde_json::to_string(&json!({
        "refs": item.refs, "requested_action": item.requested_action,
        "owner_quote": item.owner_words.as_ref().map(|w| &w.quote), "body": item.body,
        "claimed": item.claimed, "outcome": item.outcome, "withdrawn": item.withdrawn,
        "history": item.history,
    }))?;
    let total = text.chars().count();
    let chunk: String = text.chars().skip(p.offset).take(limit).collect();
    let next = p.offset.saturating_add(chunk.chars().count());
    Ok(json!({"item": summary(&item),
        "outcome": item.outcome.as_ref().map(|o| name(&o.resolution)),
        "content_json": chunk, "total_chars": total,
        "next_offset": (next < total).then_some(next)}))
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
