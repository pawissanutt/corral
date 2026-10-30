//! hello: proves the generation's one-shot bind nonce and records the digests of both
//! directional secrets, the Pi session and the host process. The nonce is then consumed.
use std::path::{Component, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{self, GenerationBound, HostHealth, ReceiptEffect, paths};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{AgentOp, CallResponse, RejectCode, Rejection};
use crate::agent::envelope::{self, Envelope};
use crate::agent::receipts::{self, Commit};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelloPayload {
    ctl_cap_sha256: String,
    session_file: PathBuf,
    pid: u32,
    start_time: String,
    pane: String,
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
    if payload.pid == 0 || !plain(&payload.start_time, 64) || !plain(&payload.pane, 128) {
        return Err(bad(
            "hello needs a pid, a start_time and a pane of [A-Za-z0-9._:+-]",
        ));
    }
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Hello)? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let selected = match &admitted.resolved {
        admission::Resolved::Task(task) => {
            agent::container_run(task, &container).and_then(|r| r.session.as_ref())
        }
        admission::Resolved::Pm(_) | admission::Resolved::Assistant(..) => None,
    };
    let sessions = selected
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
    if let Some(selected) = selected
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
    let launched = admitted
        .resolved
        .state(&container)
        .and_then(|state| state.generation(n))
        .map(|g| g.pane.clone());
    if launched.as_deref() != Some(payload.pane.as_str()) {
        return Err(Rejection::new(
            RejectCode::PaneMismatch,
            "hello came from a pane other than the generation's",
        )
        .into());
    }
    let role = envelope.identity.role;
    let bound = GenerationBound {
        host_cap_sha256: paths::sha256_hex(envelope.auth.host_cap.as_bytes()),
        ctl_cap_sha256: payload.ctl_cap_sha256,
        session_id: envelope.identity.session_id.clone(),
        session_file: payload.session_file,
        pid: payload.pid,
        start_time: payload.start_time,
        request_id: envelope.request_id.clone(),
        at: Timestamp::now(),
    };
    let key = container.key();
    // The host's herdr sequence range starts here (`agent::herdr_status`).
    let epoch = bound.at.as_millisecond();
    receipts::commit(ctx, &admitted, move |state, _| {
        // A resumed host projects its still-pending questions from the start.
        let questions: Vec<_> = state
            .questions
            .iter()
            .filter(|q| q.recorded_pending())
            .map(
                |q| json!({"key": q.key, "kind": q.kind, "blocking": q.blocking, "state": q.state}),
            )
            .collect();
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
        Ok(Commit {
            result: json!({
                "generation": n, "container": key, "role": role.as_str(),
                "ops": AgentOp::implemented_for(role).into_iter()
                    .filter(|op| *op != "todo" || todo).collect::<Vec<_>>(),
                "herdr_seq_epoch_ms": epoch, "questions": questions,
            }),
            effect: Some(ReceiptEffect {
                kind: "generation_bound".into(),
                reference: format!("g{n}"),
            }),
            post_pending: vec!["event:agent_bound".into()],
        })
    })
}
