//! Read-only worktree browser projections. No absolute host paths are sent.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Inclusive size limit for the HTML chunk selector, not the regular preview.
pub const SESSION_HTML_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Decoded bytes per chunk; leaves room for base64 and the RPC envelope within CONTROL.
pub const SESSION_HTML_CHUNK_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionFiles {
    pub entries: Vec<SessionFileEntry>,
    /// The scan or response bound was reached, so entries may be missing.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionFileEntry {
    /// One UTF-8 path component, not a path.
    pub name: String,
    pub directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionFile {
    pub kind: SessionFileKind,
    /// Base64 of the complete contents (at most 32 KiB before encoding); empty for unsupported files.
    pub data: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionFileChunk {
    /// Total file size for this revision (at most 64 MiB).
    pub size: u64,
    pub offset: u64,
    /// Opaque revision tied to the selected file. Discard the download on a stale revision.
    pub revision: String,
    /// Standard base64 of `min(size - offset, SESSION_HTML_CHUNK_BYTES)` raw bytes.
    /// EOF is reached when offset plus the decoded length equals size.
    /// A consumer opening this as HTML should validate the assembled stream as UTF-8.
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionFileKind {
    Text,
    Markdown,
    Html,
    Png,
    Jpeg,
    Binary,
    TooLarge,
    #[serde(other)]
    Other,
}
