//! Handlers of the ops request-receipts lands (hello, event and receipt_lookup), and the role
//! modules integration-seams declares for the rest: `worker` (worker-role), `reviewer`
//! (reviewer-role) and `pm` (pm-role). `call.rs` routes every op to one of them.
pub(crate) mod annotations;
pub(crate) mod assistant;
pub(crate) mod assistant_workflow;
mod auxiliary;
mod budget;
mod event;
mod hello;
pub(crate) mod jobs;
mod lookup;
pub(crate) mod memory;
pub(crate) mod notes;
pub(crate) mod owner_action;
mod owner_input;
pub(crate) mod planning;
pub(crate) mod pm;
pub(crate) mod pm_actions;
mod pm_bound;
mod pm_inbox;
pub(crate) mod reviewer;
pub(crate) mod todo;
pub(crate) mod worker;
pub(crate) mod write_permission;

#[cfg(test)]
mod payload_tests;

use anyhow::Result;
use serde::de::DeserializeOwned;

pub(crate) use event::event;
pub(crate) use hello::hello;
pub(crate) use lookup::receipt_lookup;

use super::call::{AgentOp, RejectCode, Rejection};
use super::envelope::{Envelope, error_category};

/// Even envelope-level annotation refusals must be observational (including a model parent).
pub(crate) fn log_parse_rejection(
    ctx: &crate::Ctx,
    response: &super::call::CallResponse,
    op: &str,
    input: &[u8],
) -> Result<()> {
    if !annotations::observational_rejection(op, input) {
        super::receipts::log_rejection(ctx, response, input)?;
    }
    Ok(())
}

pub(crate) fn role_not_allowed(op: AgentOp, role: corral_core::agent::AgentRole) -> anyhow::Error {
    if op == AgentOp::ReadAnnotations {
        return annotations::role_refusal();
    }
    Rejection::new(
        RejectCode::OpNotAllowed,
        format!("role {} may not call {}", role.as_str(), op.as_str()),
    )
    .into()
}

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

/// Keep serde's input-echoing message private. Only schema-owned field names and the
/// accepted shape are returned; the native tool schema supplies detailed type constraints.
pub(super) fn payload<T: DeserializeOwned>(envelope: &Envelope, expected: &str) -> Result<T> {
    serde_path_to_error::deserialize(envelope.payload.clone()).map_err(|error| {
        let message = error.inner().to_string();
        let path = error.path().to_string();
        let missing = message
            .strip_prefix("missing field `")
            .and_then(|s| s.split('`').next());
        let candidate = missing.unwrap_or_else(|| path.split('.').next().unwrap_or(""));
        // Unknown/map keys may contain secrets, so never return a path not in the fixed
        // operation field list. Nested record errors still name their safe outer field.
        let field = expected
            .split(',')
            .map(str::trim)
            .map(|s| s.trim_end_matches('?'))
            .find(|name| *name == candidate);
        let category = error_category(error.inner());
        let mut rejection = Rejection::new(
            RejectCode::BadRequest,
            format!(
                "malformed {} payload ({category} error) at {}; expected fields: {expected}",
                envelope.op,
                field.unwrap_or("payload")
            ),
        );
        rejection.detail = Some(serde_json::json!({
            "field": field, "category": category, "expected_fields": expected,
        }));
        rejection.into()
    })
}
