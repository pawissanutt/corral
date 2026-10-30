//! Handlers of the ops request-receipts lands (hello, event and receipt_lookup), and the role
//! modules integration-seams declares for the rest: `worker` (worker-role), `reviewer`
//! (reviewer-role) and `pm` (pm-role). `call.rs` routes every op to one of them.
pub(crate) mod assistant;
mod auxiliary;
mod budget;
mod event;
mod hello;
pub(crate) mod jobs;
mod lookup;
pub(crate) mod planning;
pub(crate) mod pm;
mod pm_inbox;
pub(crate) mod reviewer;
pub(crate) mod todo;
pub(crate) mod worker;

use anyhow::Result;
use serde::de::DeserializeOwned;

pub(crate) use event::event;
pub(crate) use hello::hello;
pub(crate) use lookup::receipt_lookup;

use super::call::{AgentOp, RejectCode, Rejection};
use super::envelope::{Envelope, error_category};

/// An op whose owning child has not landed: refused before any effect, naming the child.
pub(crate) fn unavailable(op: AgentOp, owner: &'static str) -> anyhow::Error {
    Rejection {
        owner: Some(owner),
        ..Rejection::new(
            RejectCode::OpUnavailable,
            format!("{} lands with T-252 child {owner}", op.as_str()),
        )
    }
    .into()
}

/// The reason names the op, serde's error category and the fixed field list, never serde's
/// message: unknown fields and variants would echo input bytes, possibly a secret.
fn payload<T: DeserializeOwned>(envelope: &Envelope, expected: &str) -> Result<T> {
    serde_json::from_value(envelope.payload.clone()).map_err(|error| {
        Rejection::new(
            RejectCode::BadRequest,
            format!(
                "malformed {} payload ({} error); expected fields: {expected}",
                envelope.op,
                error_category(&error)
            ),
        )
        .into()
    })
}
