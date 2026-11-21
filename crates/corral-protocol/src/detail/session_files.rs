//! Read-only worktree browser projections. No absolute host paths are sent.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
