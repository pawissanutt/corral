//! External extension protocol v1 (draft until OD9): manifest, framing, envelope, errors,
//! attempt states, context binding and public-read cursors. Pure types and codecs; no I/O
//! beyond the caller's reader and writer. Specification: docs/extensions-protocol.md.

pub mod attempt;
pub mod binding;
pub mod cursor;
pub mod envelope;
pub mod error;
pub mod frame;
pub mod manifest;

pub use attempt::{AttemptState, PROCESS_LOST, TerminalOutcome};
pub use binding::{
    BINDING_VERSION, Binding, BindingScope, BindingTask, CodeBinding, ContextBudget,
    ContextRequest, Freshness, PolicyBinding, ProviderBinding, TaskContextParams, WorkflowBinding,
    canonical_json, sha256_tag,
};
pub use cursor::{CURSOR_VERSION, Cursor, PROJECTION_VERSION};
pub use envelope::{
    Body, CapabilityChoice, CapabilityOffer, Envelope, EnvelopeKind, HostInfo, InitializeParams,
    InitializeResult, Method, PROTOCOL_VERSION, Progress, capability_id,
};
pub use error::{ErrorCode, ExtError};
pub use frame::{FrameReader, MAX_FRAME_BYTES, write_envelope, write_frame};
pub use manifest::{
    CursorKind, ExecutionMode, MANIFEST_FILE, MANIFEST_VERSION, Manifest, ManifestError,
    NotActivatable, PUBLIC_READS, Provider, RequestedAction, Subscription, is_extension_id,
};
