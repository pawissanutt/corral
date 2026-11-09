//! Advisory local reviews: one immutable snapshot, one auxiliary invocation, no board authority.
//! All mutations run inside commit_assistant. A lost host never restarts inference.
use anyhow::{Context, Result};
use corral_core::Store;
use corral_core::assistant::{AssistantContainer, OwnerInputSource};
use corral_core::assistant_workflow::*;
use corral_core::blob_ref::BlobHash;
use serde::Deserialize;
use serde_json::{Value, json};

use super::publication::{artifact, load_artifact};
use super::{Expected, bad, compare, current};

mod events;
mod snapshot;
pub use events::{Association, Event, accounting, apply_event};

const RECORDS: &str = "assistant-local-reviews";
fn count(store: &Store, session: &str, workflow: &str) -> Result<u32> {
    Ok(store
        .record(
            "assistant-local-review-count",
            &super::key(session, workflow),
        )?
        .unwrap_or(0))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Start {
        expected: Expected,
        input: String,
    },
    Read {
        review: Option<String>,
        #[serde(default)]
        offset: usize,
    },
    Cancel {
        expected: Expected,
        review: String,
    },
}

fn row(session: &str, workflow: &str, review: &str) -> String {
    format!("{session}/{workflow}/{review}")
}
fn load(store: &Store, session: &str, workflow: &str, review: &str) -> Result<LocalReview> {
    if !review
        .strip_prefix("LR")
        .is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n > 0 && n <= HISTORY_MAX))
    {
        return Err(bad("invalid local review ID"));
    }
    store
        .record(RECORDS, &row(session, workflow, review))?
        .context("unknown local review")
}
fn save(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    review: &mut LocalReview,
) -> Result<()> {
    review.result_revision = review
        .result_revision
        .checked_add(1)
        .context("review revision overflow")?;
    let reference = artifact(
        store,
        session,
        &review.workflow,
        &serde_json::to_vec(review)?,
    )?;
    let w = &mut container
        .workflow
        .as_mut()
        .context("workflow missing")?
        .current;
    let previous: Option<LocalReview> =
        store.record(RECORDS, &row(session, &review.workflow, &review.id))?;
    let prior_hash = previous
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()?
        .map(|v| BlobHash::of(&v));
    if let Some(index) =
        prior_hash.and_then(|hash| w.reviews.iter().position(|a| a.blob.hash() == &hash))
    {
        w.reviews[index] = reference;
    } else if previous.is_none() {
        w.reviews.push(reference);
    } else {
        return Err(bad("recovery_required: local review reference missing"));
    }
    w.state_revision = w
        .state_revision
        .checked_add(1)
        .context("workflow revision overflow")?;
    store.save_record(RECORDS, &row(session, &review.workflow, &review.id), review)?;
    crate::agent::failpoint("local_review_staged");
    Ok(())
}

pub fn start(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    expected: &Expected,
    input: &str,
) -> Result<Value> {
    let w = current(container)?;
    // Request provenance is the durable dedup key, not the bounded transport receipt window.
    let issued = count(store, session, &w.id)?;
    let request_key = format!("{session}/{}/{}", w.id, BlobHash::of(input.as_bytes()));
    if let Some(id) = store.record::<String>("assistant-local-review-requests", &request_key)? {
        return projection(
            store,
            session,
            container,
            &load(store, session, &w.id, &id)?,
        );
    }
    if issued > 0
        && !load(store, session, &w.id, &format!("LR{issued}"))?
            .lifecycle
            .terminal()
    {
        return Err(bad(
            "local review active; wait or explicitly cancel it before a new request",
        ));
    }
    compare(container, expected)?;
    let w = current(container)?;
    if w.phase != WorkflowPhase::Planning || w.owner != ScopeOwner::Local {
        return Err(bad("local review requires settled local planning"));
    }
    let plan = w
        .current_plan
        .clone()
        .ok_or_else(|| bad("publish a draft before review"))?;
    if issued >= HISTORY_MAX || w.reviews.len() >= HISTORY_MAX as usize {
        return Err(bad("local review history capacity reached"));
    }
    let evidence = container
        .owner_input(input)
        .ok_or_else(|| bad("review requires genuine owner request evidence"))?
        .clone();
    if !matches!(
        evidence.source,
        OwnerInputSource::Pane | OwnerInputSource::Dialog
    ) || evidence.generation != container.state.binding.generation
        || evidence.truncated
        || BlobHash::of(evidence.text.as_bytes()).as_str() != evidence.sha256
        || evidence.text.trim().is_empty()
    {
        return Err(bad(
            "review requires complete current-generation owner evidence",
        ));
    }
    let id = format!("LR{}", issued + 1);
    let workflow_key = super::key(session, &w.id);
    let invocation = BlobHash::of(format!("{session}/{}/{id}", w.id).as_bytes()).to_string();
    let source = snapshot::observe(store, &plan)?;
    let context = snapshot::context(store, session, container, &plan)?;
    let mut r = LocalReview {
        id,
        workflow: w.id.clone(),
        result_revision: 0,
        plan,
        context,
        request: evidence,
        invocation,
        generation: container.state.binding.generation,
        lifecycle: LocalReviewState::Prepared,
        accounting: "not_started".into(),
        model: None,
        findings: None,
        source,
        summary: String::new(),
        notice_delivered: false,
    };
    // Retain a complete packet before reservation; refs are blobs, never transcript offsets.
    snapshot::materialize(store, session, container, &r)?;
    save(store, session, container, &mut r)?;
    store.save_record("assistant-local-review-count", &workflow_key, &(issued + 1))?;
    store.save_record("assistant-local-review-requests", &request_key, &r.id)?;
    projection(store, session, container, &r)
}

pub fn cancel(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    expected: &Expected,
    id: &str,
) -> Result<Value> {
    compare(container, expected)?;
    let mut r = load(store, session, &current(container)?.id, id)?;
    if !r.lifecycle.terminal() {
        r.lifecycle = LocalReviewState::Cancelled;
        r.summary = "Explicitly cancelled; a late result cannot reopen this review.".into();
        save(store, session, container, &mut r)?;
    }
    projection(store, session, container, &r)
}

/// Handoff freezes the original packet, then closes the live local child lifetime. The frozen
/// package may honestly say running; canonical readback retains this later cancellation.
pub fn cancel_active(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
) -> Result<()> {
    let w = current(container)?;
    let issued = count(store, session, &w.id)?;
    if issued > 0 {
        let mut r = load(store, session, &w.id, &format!("LR{issued}"))?;
        if !r.lifecycle.terminal() {
            r.lifecycle = LocalReviewState::Cancelled;
            r.summary =
                "Local review cancelled by explicit handoff; frozen snapshot retained.".into();
            save(store, session, container, &mut r)?;
        }
    }
    Ok(())
}

pub fn read(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    id: Option<&str>,
    offset: usize,
) -> Result<Value> {
    let Some(index) = &container.workflow else {
        return Ok(json!({"reviews": [], "active": null}));
    };
    let w = &index.current;
    if let Some(id) = id {
        let r = load(store, session, &w.id, id)?;
        let mut result = projection(store, session, container, &r)?;
        if let Some(reference) = &r.findings {
            let bytes = load_artifact(store, reference)?;
            let text = std::str::from_utf8(&bytes)?;
            if offset > text.len() || !text.is_char_boundary(offset) {
                return Err(bad("invalid findings offset"));
            }
            let mut end = text.len().min(offset.saturating_add(1024));
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            result["text"] = json!(&text[offset..end]);
            result["next_offset"] = json!((end < text.len()).then_some(end));
        }
        return Ok(result);
    }
    // Bounded current projection; callers page exact older IDs independently.
    let issued = count(store, session, &w.id)?;
    let mut reviews = Vec::new();
    for i in issued.saturating_sub(2) + 1..=issued {
        reviews.push(projection(
            store,
            session,
            container,
            &load(store, session, &w.id, &format!("LR{i}"))?,
        )?);
    }
    Ok(json!({"reviews": reviews, "count": issued}))
}

fn projection(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    r: &LocalReview,
) -> Result<Value> {
    let w = current(container)?;
    let current_revision = w.current_plan.as_ref().map(|p| p.revision);
    let current = w
        .current_plan
        .as_ref()
        .is_some_and(|p| p.revision == r.plan.revision && p.artifact == r.plan.artifact);
    let source_current = snapshot::observe(store, &r.plan)?;
    Ok(json!({
        "review": r.id, "workflow": r.workflow, "result_revision": r.result_revision,
        "revision": r.plan.revision, "digest": r.plan.artifact.blob.hash(), "stage": r.plan.stage,
        "current_revision": current_revision, "freshness": if current {"current"} else {"stale"},
        "source_freshness": match (&r.source, &source_current) { (None, _) => "not_observed", (Some(a), Some(b)) if a == b => "current", _ => "changed" },
        "source": r.source, "lifecycle": r.lifecycle, "accounting": r.accounting,
        "invocation": r.invocation, "generation": r.generation, "model": r.model,
        "findings": r.findings, "summary": r.summary, "notice_delivered": r.notice_delivered,
        "snapshot_file": snapshot::file(store, session, r), "plan": r.plan.artifact,
        "context_file": snapshot::file(store, session, r).with_extension("context.json"), "context": r.context,
        "advisory": true,
    }))
}
