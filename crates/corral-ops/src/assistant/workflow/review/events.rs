//! Authenticated host lifecycle. Models cannot claim invocations, publish findings or replay
//! a prepared child after recovery. Auxiliary accounting and review phase change atomically.
use super::super::publication::{artifact, load_artifact};
use super::{bad, current, load, projection, save};
use anyhow::{Context, Result};
use corral_core::{
    Store,
    agent::{AuxiliaryOutcome, ContainerRef, UsageSession, UsageSessionKind, paths},
    assistant::AssistantContainer,
    assistant_workflow::*,
    blob_ref::BlobHash,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Association {
    pub workflow: String,
    pub review: String,
    pub digest: BlobHash,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Claim,
    Stage {
        offset: usize,
        chunk: String,
    },
    Settle {
        lifecycle: LocalReviewState,
        summary: String,
        bytes: Option<u32>,
        digest: Option<BlobHash>,
    },
    Recover,
    Delivered {
        result_revision: u32,
    },
}

pub fn apply_event(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    generation: u32,
    association: &Association,
    request: &str,
    event: Event,
) -> Result<Value> {
    let w = current(container)?;
    if w.id != association.workflow {
        return Err(bad("review workflow changed"));
    }
    let mut r = load(store, session, &w.id, &association.review)?;
    if r.invocation != request || r.plan.artifact.blob.hash() != &association.digest {
        return Err(bad("review invocation/snapshot mismatch"));
    }
    let mut changed = true;
    let mut claimed = false;
    match event {
        Event::Claim => {
            if r.generation != generation || r.lifecycle != LocalReviewState::Prepared {
                return Ok(
                    json!({"claimed": false, "review": projection(store, session, container, &r)?}),
                );
            }
            if w.phase != WorkflowPhase::Planning || w.owner != ScopeOwner::Local {
                return Err(bad("local review is no longer admissible"));
            }
            super::snapshot::materialize(store, session, container, &r)?;
            r.lifecycle = LocalReviewState::Queued;
            claimed = true;
        }
        Event::Stage { offset, chunk } => {
            if r.generation != generation || r.lifecycle.terminal() {
                return Ok(projection(store, session, container, &r)?);
            }
            if chunk.is_empty() || chunk.len() > CHUNK_MAX {
                return Err(bad("review chunks require 1-16 KiB UTF-8 bytes"));
            }
            let key = format!("{session}/{}/{}", r.workflow, r.id);
            let mut text: String = store
                .record("assistant-review-staging", &key)?
                .unwrap_or_default();
            let end = offset
                .checked_add(chunk.len())
                .ok_or_else(|| bad("invalid review offset"))?;
            if offset < text.len() {
                if text.as_bytes().get(offset..end) != Some(chunk.as_bytes()) {
                    return Err(bad("review chunk retry differs"));
                }
            } else if offset == text.len() && end <= DOCUMENT_MAX {
                text.push_str(&chunk);
            } else {
                return Err(bad("review chunk offset/size invalid"));
            }
            store.save_record("assistant-review-staging", &key, &text)?;
            return Ok(json!({"staged_bytes": text.len()}));
        }
        Event::Settle {
            lifecycle,
            summary,
            bytes,
            digest,
        } => {
            if r.generation != generation || !lifecycle.terminal() || summary.len() > 1024 {
                return Err(bad("invalid review terminal event"));
            }
            if r.lifecycle.terminal() {
                return Ok(projection(store, session, container, &r)?);
            }
            if lifecycle == LocalReviewState::Completed
                && (r.lifecycle != LocalReviewState::Running || r.accounting != "completed")
            {
                return Err(bad(
                    "completed review needs a settled completed auxiliary invocation",
                ));
            }
            let key = format!("{session}/{}/{}", r.workflow, r.id);
            if let (Some(bytes), Some(digest)) = (bytes, digest.as_ref()) {
                let text: String = store
                    .record("assistant-review-staging", &key)?
                    .context("no staged findings")?;
                if text.len() != bytes as usize || BlobHash::of(text.as_bytes()) != *digest {
                    return Err(bad("review result digest/length mismatch"));
                }
                if text.trim().is_empty() && lifecycle == LocalReviewState::Completed {
                    return Err(bad("empty review is incomplete"));
                }
                r.findings = Some(artifact(store, session, &r.workflow, text.as_bytes())?);
            } else if bytes.is_some()
                || digest.is_some()
                || lifecycle == LocalReviewState::Completed
            {
                return Err(bad("completed review needs exact findings"));
            }
            r.lifecycle = lifecycle;
            r.summary = summary;
            store.delete_record("assistant-review-staging", &key)?;
        }
        Event::Recover => {
            if !r.lifecycle.terminal() {
                r.lifecycle = LocalReviewState::Interrupted;
                r.summary =
                    "Host lost/reloaded before durable result publication; no automatic replay."
                        .into();
            } else {
                changed = false;
            }
            if let Some(usage) = container
                .state
                .usage_sessions
                .iter_mut()
                .find(|u| u.request.as_deref() == Some(request))
            {
                match usage.outcome {
                    Some(AuxiliaryOutcome::Prepared) => {
                        usage.outcome = Some(AuxiliaryOutcome::NotStarted);
                        r.accounting = "not_started".into();
                        changed = true;
                    }
                    Some(AuxiliaryOutcome::Running) => {
                        usage.outcome = Some(AuxiliaryOutcome::Uncertain);
                        r.accounting = "uncertain".into();
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        Event::Delivered { result_revision } => {
            if !r.lifecycle.terminal() || result_revision != r.result_revision {
                return Err(bad("stale local review notice"));
            }
            r.notice_delivered = true;
            save(store, session, container, &mut r)?;
            return projection(store, session, container, &r);
        }
    }
    if changed {
        save(store, session, container, &mut r)?;
    }
    let mut result = projection(store, session, container, &r)?;
    result["claimed"] = json!(claimed);
    Ok(result)
}

pub fn accounting(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    generation: u32,
    association: &Association,
    request: &str,
    action: &str,
    model: Option<&str>,
    outcome: Option<AuxiliaryOutcome>,
    max: usize,
) -> Result<Value> {
    let mut r = load(store, session, &association.workflow, &association.review)?;
    if current(container)?.id != r.workflow
        || r.generation != generation
        || r.invocation != request
        || r.plan.artifact.blob.hash() != &association.digest
    {
        return Err(bad("review accounting association differs"));
    }
    let file = paths::usage_session_file(
        store,
        &ContainerRef::Assistant {
            session: session.into(),
        },
        request,
    );
    let position = container
        .state
        .usage_sessions
        .iter()
        .position(|u| u.request.as_deref() == Some(request));
    match action {
        "prepare" => {
            if r.lifecycle != LocalReviewState::Queued
                || position.is_some()
                || container.state.usage_sessions.len() >= 4096
            {
                return Err(bad("review auxiliary already prepared or unavailable"));
            }
            let model = model
                .filter(|m| !m.is_empty() && m.len() <= 256 && !m.chars().any(char::is_control))
                .ok_or_else(|| bad("review requires configured explore model"))?;
            r.model = Some(model.into());
            container.state.usage_sessions.push(UsageSession {
                generation,
                kind: UsageSessionKind::Explore,
                session_id: request.into(),
                session_file: file.clone(),
                model: Some(model.into()),
                request: Some(request.into()),
                outcome: Some(AuxiliaryOutcome::Prepared),
            });
        }
        "started" => {
            let live = container
                .state
                .usage_sessions
                .iter()
                .filter(|u| {
                    u.generation == generation
                        && u.kind == UsageSessionKind::Explore
                        && u.outcome == Some(AuxiliaryOutcome::Running)
                })
                .count();
            let u = &mut container.state.usage_sessions
                [position.context("unknown review auxiliary")?];
            if r.lifecycle != LocalReviewState::Queued
                || u.outcome != Some(AuxiliaryOutcome::Prepared)
                || live >= max
            {
                return Err(bad("review start refused: phase/parallel limit"));
            }
            u.outcome = Some(AuxiliaryOutcome::Running);
            r.lifecycle = LocalReviewState::Running;
            r.accounting = "uncertain".into();
        }
        "finish" => {
            let u = &mut container.state.usage_sessions
                [position.context("unknown review auxiliary")?];
            let allowed = match u.outcome {
                Some(AuxiliaryOutcome::Prepared) => outcome == Some(AuxiliaryOutcome::NotStarted),
                Some(AuxiliaryOutcome::Running) => matches!(
                    outcome,
                    Some(AuxiliaryOutcome::Completed | AuxiliaryOutcome::Uncertain)
                ),
                _ => false,
            };
            if !allowed {
                return Err(bad("review auxiliary already terminal or invalid"));
            }
            u.outcome = outcome.clone();
            r.accounting = match outcome {
                Some(AuxiliaryOutcome::Completed) => "completed",
                Some(AuxiliaryOutcome::NotStarted) => "not_started",
                _ => "uncertain",
            }
            .into();
        }
        _ => return Err(bad("unknown review accounting action")),
    }
    if let Some(a) = &r.findings {
        load_artifact(store, a)?;
    }
    save(store, session, container, &mut r)?;
    Ok(json!({"session_file": file, "session_id": request, "generation": generation}))
}
