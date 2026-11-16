//! Peer operations are available to every bound role. Only authenticated container identity
//! supplies attribution; publication is informational and never uses owner admission.
use anyhow::Result;
use corral_core::inbox::AgentPostRefusal;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::{self, Admission, Admitted, Mode, Resolved};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::Envelope;
use crate::agent::receipts::{self, Commit};
use crate::delivery::peer;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    to: String,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn finish(ctx: &Ctx, admitted: &Admitted, result: Value) -> Result<CallResponse> {
    let commit = || {
        Ok(Commit {
            result,
            effect: None,
            post_pending: Vec::new(),
        })
    };
    if admitted.origin.binding.container.task().is_some() {
        receipts::commit_head(ctx, admitted, |_| commit())
    } else {
        receipts::commit(ctx, admitted, |_, _| commit())
    }
}

pub(crate) fn send(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let payload: Send = super::payload(envelope, "to, text")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let sender = peer::recipient(&admitted.origin.binding.container);
    let self_key = sender.key()?;
    if payload.to == self_key {
        return finish(
            ctx,
            &admitted,
            json!({"state":"undeliverable", "detail":"self_send: cannot send to yourself"}),
        );
    }
    let Some(recipient) = peer::lookup(ctx, &payload.to)? else {
        return finish(
            ctx,
            &admitted,
            json!({"state":"undeliverable", "detail":"recipient_unknown_or_closed"}),
        );
    };
    let assistant_title = match &sender {
        corral_core::inbox::Recipient::Assistant { session } => {
            Some(ctx.store.get_assistant(session)?.summary)
        }
        _ => None,
    };
    let title = match &admitted.resolved {
        Resolved::Task(head) => Some(head.title.as_str()),
        Resolved::Assistant(_, _) => assistant_title.as_deref().filter(|title| !title.is_empty()),
        Resolved::Pm(_) => None,
    };
    let message = match peer::post(
        ctx,
        peer::PostRequest {
            recipient: &recipient,
            sender: &sender,
            role: admitted.origin.identity.role,
            title,
            generation: admitted.origin.binding.generation,
            request_id: &envelope.request_id,
            payload_sha256: &envelope.payload_sha256,
            text: &payload.text,
        },
    ) {
        Ok(message) => message,
        Err(error) => match error.downcast_ref::<AgentPostRefusal>() {
            Some(AgentPostRefusal::Limit) => {
                return Err(Rejection::new(
                    RejectCode::AgentMessageLimit,
                    "at most 60 agent messages per run generation",
                )
                .into());
            }
            Some(AgentPostRefusal::PayloadMismatch) => {
                return Err(Rejection::new(
                    RejectCode::PayloadMismatch,
                    "peer request was already used with a different payload",
                )
                .into());
            }
            Some(AgentPostRefusal::RecipientClosed) => {
                return finish(
                    ctx,
                    &admitted,
                    json!({"state":"undeliverable", "detail":"recipient_closed"}),
                );
            }
            None if error.to_string().starts_with("message size ") => {
                return Err(Rejection::new(
                    RejectCode::BadRequest,
                    "agent message including framing exceeds the 8 KiB bridge input limit",
                )
                .into());
            }
            None => return Err(error),
        },
    };
    let result = peer::kick(ctx, &message, &crate::agent::delivery::CorralPiAdapter)?;
    finish(ctx, &admitted, result)
}

pub(crate) fn roster(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let _: Empty = super::payload(envelope, "")?;
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    let self_key = peer::recipient(&admitted.origin.binding.container).key()?;
    let peers = peer::roster(ctx, &self_key)?;
    finish(ctx, &admitted, json!({"self": self_key, "peers": peers}))
}
