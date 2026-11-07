use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::assistant::AssistantContainer;
use corral_core::assistant_workflow::{DOCUMENT_MAX, PlanRevision};
use serde_json::{Value, json};

use super::publication::{Staging, load_artifact};
use super::{REVISIONS, STAGING, bad, current, key, revision_key, summary};

/// Receipt-free projection. History is key-paged, never loaded as an accumulated vector.
/// The complete JSON result (not just document text) stays within the receipt result bound.
pub fn read_state(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    after: u32,
    limit: u32,
) -> Result<Value> {
    if !(1..=8).contains(&limit) {
        return Err(bad("history limit must be 1-8"));
    }
    let mut result = summary(container)?;
    result["boundary"] = super::boundary::projection(store, session)?;
    let Some(index) = &container.workflow else {
        return Ok(result);
    };
    let w = &index.current;
    // Missing/corrupt canonical bytes are recovery-required, never ordinary mode.
    load_artifact(store, &w.original_request)?;
    if let Some(plan) = &w.current_plan {
        load_artifact(store, &plan.artifact)?;
        for reference in plan.execution_scope.iter().chain(plan.assumptions.iter()) {
            load_artifact(store, reference)?;
        }
    }
    let last = w.current_plan.as_ref().map_or(0, |p| p.revision);
    if after > last {
        return Err(bad("history cursor exceeds the current revision"));
    }
    let staged: Option<Staging> = store.record(STAGING, &key(session, &w.id))?;
    result["staged_bytes"] = json!(staged.map_or(0, |s| s.text.len()));
    result["revisions"] = json!([]);
    result["next_after"] = Value::Null;
    let mut cursor = after;
    for revision in (after + 1)..=last.min(after.saturating_add(limit)) {
        let row: PlanRevision = store
            .record(REVISIONS, &revision_key(session, &w.id, revision))?
            .context("recovery_required: design history missing")?;
        let mut candidate = result.clone();
        candidate["revisions"]
            .as_array_mut()
            .context("history projection")?
            .push(json!(row));
        candidate["next_after"] = if revision < last {
            json!(revision)
        } else {
            Value::Null
        };
        if serde_json::to_vec(&candidate)?.len() > corral_core::agent::RESULT_MAX_BYTES {
            anyhow::ensure!(
                cursor != after,
                "recovery_required: design history metadata exceeds read bound"
            );
            break;
        }
        result = candidate;
        cursor = revision;
    }
    if cursor < last {
        result["next_after"] = json!(cursor);
    }
    Ok(result)
}

/// Exact UTF-8 pages of the original request (revision 0) or a published revision. The page
/// boundary never splits a codepoint. The small byte cap also bounds JSON escape expansion.
pub fn read_revision(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    revision: u32,
    offset: usize,
    limit: usize,
) -> Result<Value> {
    if !(1..=1024).contains(&limit) || offset > DOCUMENT_MAX {
        return Err(bad(
            "revision read limit must be 1-1024 bytes with a bounded offset",
        ));
    }
    let w = current(container)?;
    let reference = if revision == 0 {
        w.original_request.clone()
    } else {
        if revision > w.current_plan.as_ref().map_or(0, |p| p.revision) {
            return Err(bad("unknown design revision"));
        }
        let row: PlanRevision = store
            .record(REVISIONS, &revision_key(session, &w.id, revision))?
            .context("recovery_required: design history missing")?;
        row.artifact
    };
    let bytes = load_artifact(store, &reference)?;
    let text = std::str::from_utf8(&bytes).context("recovery_required: design is not UTF-8")?;
    if !text.is_char_boundary(offset) {
        return Err(bad("offset is not a UTF-8 byte boundary"));
    }
    let mut end = bytes.len().min(offset.saturating_add(limit));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == offset && offset < bytes.len() {
        return Err(bad("limit cannot hold the next UTF-8 codepoint"));
    }
    Ok(
        json!({"workflow": w.id, "revision": revision, "artifact": reference,
        "offset": offset, "text": &text[offset..end],
        "next_offset": if end < bytes.len() { Some(end) } else { None }}),
    )
}
