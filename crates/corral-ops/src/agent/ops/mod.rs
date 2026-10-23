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
use super::envelope::Envelope;

fn payload<T: DeserializeOwned>(envelope: &Envelope) -> Result<T> {
    serde_json::from_value(envelope.payload.clone()).map_err(|error| {
        let message = error.to_string();
        let reason = if message.contains('"') {
            format!("malformed {} payload", envelope.op)
        } else {
            format!("malformed {} payload: {message}", envelope.op)
        };
        Rejection::new(RejectCode::BadRequest, reason).into()
    })
}
