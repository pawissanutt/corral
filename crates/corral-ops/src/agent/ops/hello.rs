//! hello: proves the generation's one-shot bind nonce and records the digests of both
//! directional secrets, the Pi session and the host process. Pane-less generations prove an
//! explicit daemon binding with no pane; terminal generations still require their exact pane.
use std::path::{Component, PathBuf};

use anyhow::{Context, Result};
use corral_core::TaskRef;
use corral_core::agent::{
    AgentRole, ContainerRef, GenerationBound, HostHealth, NativeRecord, ReceiptEffect, paths,
};
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{AgentOp, CallResponse, RejectCode, Rejection};
use crate::agent::envelope::{self, Envelope};
use crate::agent::herdr_status;
use crate::agent::receipts::{self, Commit};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloPayload {
    #[serde(default)]
    binding_digest: Option<String>,
    ctl_cap_sha256: String,
    session_file: PathBuf,
    pid: u32,
    start_time: String,
    #[serde(default)]
    pane: Option<String>,
}

fn bad(reason: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn plain(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:+-".contains(c))
}

pub(crate) fn hello(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: HelloPayload = super::payload(
        envelope,
        "ctl_cap_sha256, session_file, pid, start_time, pane",
    )?;
    envelope::hex64("payload.ctl_cap_sha256", &payload.ctl_cap_sha256)?;
    if payload.ctl_cap_sha256 == paths::sha256_hex(envelope.auth.host_cap.as_bytes()) {
        return Err(bad("host_cap and ctl_cap must be different secrets"));
    }
    if payload.pid == 0
        || !plain(&payload.start_time, 64)
        || payload
            .pane
            .as_deref()
            .is_some_and(|pane| !plain(pane, 128))
    {
        return Err(bad(
            "hello needs a pid, a start_time and a valid pane when present",
        ));
    }
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Hello)? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let selected = match &admitted.resolved {
        admission::Resolved::Task(task) => match &container {
            ContainerRef::Worker { run, .. } => task.run_head(run)?.and_then(|r| r.session),
            ContainerRef::Reviewer { run, review, .. } => {
                task.review_child(run, review)?.and_then(|r| r.session)
            }
            _ => None,
        },
        admission::Resolved::Pm(_) | admission::Resolved::Assistant(..) => None,
    };
    let sessions = selected
        .as_ref()
        .map(|s| s.directory.clone())
        .unwrap_or_else(|| paths::container_sessions_dir(&ctx.store, &container));
    let session = &payload.session_file;
    if session.as_os_str().len() > 4096
        || !session.is_absolute()
        || session.components().any(|c| c == Component::ParentDir)
        || session.parent() != Some(sessions.as_path())
    {
        return Err(bad(
            "session_file must be an absolute path under the container's sessions directory",
        ));
    }
    if let Some(selected) = selected.as_ref()
        && matches!(
            selected.action,
            corral_core::SessionAction::Reused | corral_core::SessionAction::Resumed
        )
        && (selected.transcript.as_ref() != Some(session)
            || selected.id.as_deref() != Some(envelope.identity.session_id.as_str()))
    {
        return Err(bad("hello did not resume the exact recorded role session"));
    }
    let n = envelope.identity.generation;
    let generation = admitted.resolved.generation(&container, n)?;
    let connection = generation.as_ref().and_then(|g| g.connection.as_ref());
    let retained_legacy =
        payload.binding_digest.is_none() && connection.is_some_and(|b| !b.requires_ack());
    if !retained_legacy
        && connection.map(|b| b.digest.as_str()) != payload.binding_digest.as_deref()
    {
        return Err(bad(
            "hello connection binding does not match the launched generation",
        ));
    }
    let generation = generation.context("the generation disappeared")?;
    let pane_matches = if generation.daemon_hosted {
        envelope.identity.role == AgentRole::Assistant && payload.pane.is_none()
    } else {
        payload.pane.as_deref() == Some(generation.pane.as_str())
    };
    if !pane_matches {
        return Err(Rejection::new(
            RejectCode::PaneMismatch,
            "hello came from a pane other than the generation's",
        )
        .into());
    }
    let role = envelope.identity.role;
    let mut bound = GenerationBound {
        host_cap_sha256: paths::sha256_hex(envelope.auth.host_cap.as_bytes()),
        ctl_cap_sha256: payload.ctl_cap_sha256,
        session_id: envelope.identity.session_id.clone(),
        session_file: payload.session_file,
        pid: payload.pid,
        start_time: payload.start_time,
        request_id: envelope.request_id.clone(),
        at: corral_core::clock::now(),
        herdr_seq_epoch: None,
    };
    let key = container.key();
    bound.herdr_seq_epoch = match herdr_status::allocate_epoch(bound.at) {
        Ok(epoch) => Some(epoch),
        Err(error) => {
            crate::messages::report(
                container.task(),
                "agent herdr status",
                format!("allocating {key}'s herdr sequence epoch: {error:#}"),
            );
            None
        }
    };
    let epoch = bound.herdr_seq_epoch;
    let continuation = crate::availability::continuation::notice(&ctx.store, &container, n)?;
    let activate_container = container.clone();
    if container.task().is_some() {
        let response = receipts::commit_head(ctx, &admitted, move |task| {
            let mut generation = task
                .agent_generation(&container, n)?
                .context("the generation disappeared")?;
            if generation.bound.is_some() {
                return Err(Rejection::new(
                    RejectCode::NonceConsumed,
                    "the bind nonce was already consumed",
                )
                .into());
            }
            generation.bound = Some(bound);
            let todo = generation.todo;
            task.put_agent_record(&container, NativeRecord::Generation(Box::new(generation)))?;
            task.edit_agent_head(&container, |head| {
                head.binding.health = HostHealth::Healthy;
                Ok(())
            })?;
            let questions = task.pending_agent_questions(&container)?;
            Ok(bound_commit(
                n,
                role,
                &key,
                todo,
                epoch,
                herdr_status::question_snapshot_rows(questions.iter(), &[]),
                continuation,
            ))
        })?;
        activate_bound(ctx, &activate_container, n);
        return Ok(response);
    }
    let response = receipts::commit(ctx, &admitted, move |state, _| {
        // A resumed host projects its still-pending questions from the start.
        let questions = herdr_status::question_snapshot(state, &[]);
        let generation = state
            .generation_mut(n)
            .context("the generation disappeared")?;
        if generation.bound.is_some() {
            return Err(Rejection::new(
                RejectCode::NonceConsumed,
                "the bind nonce was already consumed",
            )
            .into());
        }
        generation.bound = Some(bound);
        let todo = generation.todo;
        state.binding.health = HostHealth::Healthy;
        Ok(bound_commit(
            n,
            role,
            &key,
            todo,
            epoch,
            questions,
            continuation,
        ))
    })?;
    activate_bound(ctx, &activate_container, n);
    Ok(response)
}

// The hello receipt is already committed. Failed activation must revoke any spend owner,
// not turn a successful bind into a retryable hello or expose a raw provider error.
fn activate_bound(ctx: &Ctx, container: &ContainerRef, generation: u32) {
    use crate::availability::continuation::{self, PauseReason};
    let Err(error) = continuation::activate(ctx, container, generation) else {
        return;
    };
    // A session fence may win the activation transaction after the successful hello.
    // Its Prepared intent is retried by owner Resume/input, not financially paused.
    if crate::operating::refused(&error)
        || matches!(
            continuation::automation_may_start(ctx, container),
            Ok(false)
        )
    {
        return;
    }
    if continuation::pause(
        ctx,
        container,
        PauseReason::SuccessorAmbiguous,
        "activation failed",
    )
    .is_err()
    {
        crate::messages::report(
            container.task(),
            "agent continuation",
            "continuation activation and pause failed".into(),
        );
    }
}

fn bound_commit(
    n: u32,
    role: AgentRole,
    key: &str,
    todo: bool,
    epoch: Option<u64>,
    questions: Vec<serde_json::Value>,
    continuation: Option<serde_json::Value>,
) -> Commit {
    Commit {
        result: json!({
            "generation": n, "container": key, "role": role.as_str(),
            "ops": AgentOp::implemented_for(role).into_iter()
                .filter(|op| *op != "todo" || todo).collect::<Vec<_>>(),
            "herdr_seq_epoch_ms": epoch, "questions": questions,
            "continuation": continuation,
        }),
        effect: Some(ReceiptEffect {
            kind: "generation_bound".into(),
            reference: format!("g{n}"),
        }),
        post_pending: vec!["event:agent_bound".into()],
    }
}

#[cfg(test)]
#[path = "hello_daemon_tests.rs"]
mod daemon_tests;
