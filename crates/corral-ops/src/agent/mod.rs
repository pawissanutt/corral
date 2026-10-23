//! Pi-native agent host request path (T-252). request-receipts owns the envelope, admission,
//! single-phase receipts, the generation binding primitives, the hello, event and
//! receipt_lookup ops and the complete v1 `corral agent` surface; later children land the
//! remaining op and subcommand handlers that `call.rs` and `cli.rs` name as unavailable.
mod admission;
#[cfg(test)]
mod admission_tests;
mod call;
pub mod cli;
mod envelope;
mod ops;
mod receipts;
#[cfg(test)]
mod receipts_tests;
#[cfg(test)]
mod test_support;

pub use admission::{BindTarget, GenerationGrant, fence_generation, open_generation};
pub use call::{AgentOp, CallResponse, RejectCode, Rejection, call};
pub use envelope::{
    ENVELOPE_MAX_BYTES, ENVELOPE_VERSION, Envelope, canonical_json, event_request_id,
    payload_sha256, tool_request_id,
};
