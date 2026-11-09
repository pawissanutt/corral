//! Receipt-free annotation reads and host-origin owner actions. No model mutation op exists.
use anyhow::Result;
use corral_core::{
    Store, TaskHead,
    agent::{AgentRole, RequestBinding},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::{
    admission::{self, Admission, Mode, Resolved},
    call::{CallResponse, RejectCode, Rejection},
    envelope::{Envelope, event_id_parts},
};
use crate::{
    Ctx,
    annotations::{self, *},
};

/// Annotation refusals, like their reads and key replays, have no store effects. Other host
/// operations retain the protocol's existing rejection journal.
pub(crate) fn handles(envelope: &Envelope) -> bool {
    envelope.op == "read_annotations"
        || (envelope.op == "event" && envelope.payload["kind"] == "owner_annotation")
}

pub(crate) fn observational_rejection(op: &str, input: &[u8]) -> bool {
    op == "read_annotations"
        || (op == "event"
            && input.len() <= crate::agent::envelope::ENVELOPE_MAX_BYTES
            && serde_json::from_slice::<Value>(input)
                .ok()
                .is_some_and(|v| v["payload"]["kind"] == "owner_annotation"))
}

pub(crate) const ROLES: &[AgentRole] = &[
    AgentRole::Worker,
    AgentRole::Reviewer,
    AgentRole::Pm,
    AgentRole::Assistant,
];

fn refused(error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<AnnotationError>() {
        Some(error) => Rejection::new(RejectCode::AnnotationRefused, &error.message)
            .with_detail(json!({"detail": error}))
            .into(),
        None => error,
    }
}
pub(crate) fn role_refusal() -> anyhow::Error {
    refused(annotation_error(
        AnnotationCode::RoleNotPermitted,
        "annotations are unavailable for this role",
        Value::Null,
    ))
}
fn bad(reason: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn scope(store: &Store, envelope: &Envelope, head: &TaskHead) -> Result<()> {
    let identity = &envelope.identity;
    let allowed = match identity.role {
        AgentRole::Worker | AgentRole::Reviewer => identity.task.as_deref() == Some(&head.id),
        AgentRole::Pm => {
            crate::agent::hooks::pm::contains(store, &identity.project, head.project.as_deref())?
        }
        AgentRole::Assistant => true,
        _ => return Err(role_refusal()),
    };
    if !allowed {
        return Err(Rejection::new(
            RejectCode::OutOfScope,
            "annotation task is outside the host scope",
        )
        .into());
    }
    Ok(())
}

/// Authenticate afresh even on entry-key replay. The service repeats this under its task/board
/// leases, which also serialize PM and assistant generation writers. Never use host receipt replay.
fn authorize(ctx: &Ctx, envelope: &Envelope, head: &TaskHead, live: bool) -> Result<()> {
    if !ROLES.contains(&envelope.identity.role) {
        return Err(role_refusal());
    }
    scope(&ctx.store, envelope, head)?;
    let admitted = match admission::admit(ctx, envelope, Mode::Lookup)? {
        Admission::New(admitted) => admitted,
        Admission::Replay(_) => {
            return Err(bad(
                "annotation calls cannot reuse an agent receipt identity",
            ));
        }
    };
    if live {
        match &admitted.resolved {
            Resolved::Task(_) => admission::recheck_task_head(
                head,
                &RequestBinding {
                    container: admission::container_of(&envelope.identity),
                    generation: envelope.identity.generation,
                    callback: envelope.identity.callback.clone(),
                    settles_turn: None,
                },
            )?,
            Resolved::Pm(pm) => admission::recheck_pm(pm, envelope.identity.generation)?,
            Resolved::Assistant(assistant, ended) => {
                admission::recheck_assistant(assistant, *ended, envelope.identity.generation)?
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    task: String,
    thread: Option<String>,
    #[serde(default)]
    query: AnnotationsQuery,
}

fn response(envelope: &Envelope, result: Value, max: usize) -> Result<CallResponse> {
    // The view budget is enforced by the service; the complete transport envelope must also
    // fit it. Reads retry with a smaller page rather than silently truncate a legal entry.
    let value = json!({"version":1, "request_id":envelope.request_id, "op":envelope.op,
        "state":"committed", "stored":false, "result":result});
    if serde_json::to_vec(&value)?.len() > max {
        return Err(bad("annotation response exceeds its encoded byte budget"));
    }
    Ok(CallResponse::Unstored { envelope: value })
}

pub(crate) fn read(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let mut input: Read = super::payload(envelope, "task, thread?, query?")?;
    let task = Store::normalize_id(&input.task).map_err(|_| bad("invalid annotation task"))?;
    authorize(ctx, envelope, &ctx.store.head(&task)?, false)?;
    loop {
        let view = match &input.thread {
            Some(thread) => annotations::show(&ctx.store, &task, thread, &input.query),
            None => annotations::list(&ctx.store, &task, &input.query),
        }
        .map_err(refused)?;
        let mut result = serde_json::to_value(view)?;
        // Candidate paths are ancillary and explicitly truncatable; never discard entries.
        while response(envelope, result.clone(), RESPONSE_BYTES).is_err() {
            let paths = result["paths"]
                .as_array_mut()
                .ok_or_else(|| bad("invalid annotation paths"))?;
            if paths.pop().is_none() {
                break;
            }
            result["paths_truncated"] = json!(true);
        }
        let answer = response(envelope, result, RESPONSE_BYTES);
        if answer.is_ok() || input.query.limit == Some(1) {
            return answer;
        }
        input.query.limit = Some(input.query.limit.unwrap_or(16).div_ceil(2));
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerEvent {
    kind: String,
    task: String,
    request_key: String,
    // Nested strict action avoids serde flatten swallowing unknown author/authority fields.
    action: AnnotationAction,
}

pub(crate) fn owner_event(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    if !ROLES.contains(&envelope.identity.role) {
        return Err(role_refusal());
    }
    if envelope.identity.parent.is_some()
        || !matches!(event_id_parts(&envelope.request_id), Some((g, _)) if g == envelope.identity.generation)
    {
        return Err(bad(
            "owner_annotation is a host-only event, not a model tool request",
        ));
    }
    let input: OwnerEvent = super::payload(envelope, "kind, task, request_key, action")?;
    if input.kind != "owner_annotation" {
        return Err(bad("expected owner_annotation"));
    }
    let actor = AnnotationActor::authenticated(|_, head| {
        authorize(ctx, envelope, head, true)?;
        Ok(AnnotationAuthor::Owner {
            version: 1,
            surface: AnnotationSurface::PiPanel {
                container: admission::container_of(&envelope.identity),
                session: envelope.identity.session_id.clone(),
                generation: u64::from(envelope.identity.generation),
            },
        })
    });
    let result = annotations::mutate(
        &ctx.store,
        &actor,
        &input.task,
        &input.request_key,
        input.action,
    )
    .map_err(refused)?;
    response(envelope, serde_json::to_value(result)?, 8 * 1024)
}

#[cfg(test)]
#[path = "annotations_tests.rs"]
mod tests;
