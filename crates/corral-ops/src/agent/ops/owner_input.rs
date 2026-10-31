//! The owner-input journal (T-544 owner-authority): the assistant host's host-only `event` op
//! `owner_input` records one pane-typed owner message, or the owner's answer to a confirmation
//! the host rendered, before the text reaches the model. `owner_action` may cite only these
//! entries; corral wakes, inbox deliveries, PM outcomes and recovery notices never become one.
use anyhow::Result;
use corral_core::agent::{AgentRole, ReceiptEffect, paths};
use corral_core::assistant::{NewOwnerInput, OWNER_INPUT_TEXT_MAX, OwnerInputSource};
use serde::Deserialize;
use serde_json::json;

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::{Envelope, event_id_parts};
use crate::agent::receipts::{self, Commit};

/// Bytes of the command line a confirmation shows.
pub const COMMAND_MAX: usize = 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    #[serde(rename = "kind")]
    _kind: String,
    source: OwnerInputSource,
    text: String,
    sha256: String,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    answers: Option<String>,
}

fn bad(reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

/// `rq-` and 32 lowercase hex digits, the shape of a tool request ID.
fn tool_request(id: &str) -> bool {
    id.strip_prefix("rq-").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn validate(envelope: &Envelope, payload: &Payload) -> Result<()> {
    if envelope.identity.role != AgentRole::Assistant {
        return Err(bad("only an assistant host journals owner input"));
    }
    match event_id_parts(&envelope.request_id) {
        Some((generation, _)) if generation == envelope.identity.generation => {}
        _ => return Err(bad("owner_input uses request_id ev-<generation>-<seq>")),
    }
    if payload.text.trim().is_empty() || payload.text.len() > OWNER_INPUT_TEXT_MAX {
        return Err(bad(format!(
            "owner input text must be 1..{OWNER_INPUT_TEXT_MAX} bytes"
        )));
    }
    let hex = payload.sha256.len() == 64
        && payload
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !hex || (!payload.truncated && payload.sha256 != paths::sha256_hex(payload.text.as_bytes()))
    {
        return Err(bad(
            "sha256 must be the lowercase digest of the complete input",
        ));
    }
    let answers = payload.answers.as_deref();
    match (payload.source, payload.command.as_deref()) {
        (OwnerInputSource::Pane, None) if answers.is_none() => Ok(()),
        (OwnerInputSource::Confirmation, Some(command))
            if matches!(payload.text.as_str(), "y" | "n")
                && !command.is_empty()
                && command.len() <= COMMAND_MAX
                && !payload.truncated
                && answers.is_some_and(tool_request) =>
        {
            Ok(())
        }
        _ => Err(bad(
            "a pane input carries no command; a confirmation carries the command it showed, the \
             answer y or n and the request it answers",
        )),
    }
}

/// event kind `owner_input`: appends the next `A-<n>/g<generation>/i<seq>` entry to the
/// session's journal in the receipt's transaction and answers with its ID. A replay returns
/// the same ID; nothing else is logged.
pub(crate) fn journal(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: Payload = serde_json::from_value(envelope.payload.clone()).map_err(|_| {
        bad("malformed owner_input payload; expected fields: kind, source, text, sha256, truncated?, command?, answers?")
    })?;
    validate(envelope, &payload)?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let session = envelope.identity.assistant.clone().unwrap_or_default();
    let input = NewOwnerInput {
        generation: envelope.identity.generation,
        turn: envelope.identity.turn,
        source: payload.source,
        sha256: payload.sha256,
        text: payload.text,
        truncated: payload.truncated,
        command: payload.command,
        answers: payload.answers,
        request: envelope.request_id.clone(),
    };
    receipts::commit_assistant(ctx, &admitted, move |assistant| {
        let entry = assistant.journal_owner_input(&session, input);
        Ok(Commit {
            result: json!({"kind": "owner_input", "input": entry.id, "source": entry.source,
                "turn": entry.turn}),
            effect: Some(ReceiptEffect {
                kind: "owner_input".into(),
                reference: entry.id,
            }),
            post_pending: Vec::new(),
        })
    })
}
