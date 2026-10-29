//! Pi-native agent host request path (T-252). request-receipts owns the envelope, admission,
//! single-phase receipts, the generation binding primitives, the hello, event and
//! receipt_lookup ops and the complete v1 `corral agent` surface; later children land the
//! remaining op and subcommand handlers that `call.rs` and `cli.rs` name as unavailable.
//! op-receipts owns `effects`: receipts inside the swept operations and the two-phase gate.
//! integration-seams declares every later module below, each documenting its owning child, and
//! owns `hooks`, the only path from existing corral code into the Pi-native host. Later
//! children fill these modules and never edit this file.
mod admission;
#[cfg(test)]
mod admission_tests;
pub mod aws_profile;
pub mod bridge;
mod call;
#[cfg(all(test, unix))]
mod capabilities_tests;
pub mod cli;
pub mod credentials;
pub mod delivery;
pub mod effects;
#[cfg(test)]
mod effects_stat_tests;
#[cfg(test)]
mod effects_tests;
mod envelope;
pub(crate) mod gate_queue;
pub mod hooks;
pub mod install;
pub mod jobs;
#[cfg(all(test, target_os = "linux"))]
mod jobs_tests;
pub mod launch;
pub(crate) mod ops;
pub mod pi;
pub mod pm_wake;
mod receipts;
#[cfg(test)]
pub(crate) use receipts::FAILPOINT;
#[cfg(test)]
mod receipts_tests;
pub mod recovery;
pub mod rewind;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod todo_tests;
pub mod turns;
pub mod usage;
pub mod wake;

pub use crate::{
    artifacts::add_artifact_request, comments::comment_request,
    completion::record_evidence_request, obligations::register_wait_request,
    progress::save_checkpoint_request, progress::update_progress_request, review::done_request,
    reviewer::verdict_request,
};
pub use admission::{BindTarget, GenerationGrant, fence_generation, open_generation};
pub use call::{AgentOp, CallResponse, RejectCode, Rejection, call};
pub(crate) use effects::failpoint;
pub use envelope::{
    ENVELOPE_MAX_BYTES, ENVELOPE_VERSION, Envelope, canonical_json, event_request_id,
    payload_sha256, tool_request_id,
};
