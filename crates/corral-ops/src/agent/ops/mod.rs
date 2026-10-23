//! Handlers of the ops request-receipts lands: hello, event and receipt_lookup.
mod event;
mod hello;
mod lookup;

use anyhow::Result;
use serde::de::DeserializeOwned;

pub(crate) use event::event;
pub(crate) use hello::hello;
pub(crate) use lookup::receipt_lookup;

use super::call::{RejectCode, Rejection};
use super::envelope::{Envelope, error_category};

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
