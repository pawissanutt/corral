//! Bounded previews from the canonical artifact catalog, never arbitrary host paths.
use corral_types::Artifact;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactPage {
    /// Owner ID: the task for an artifact, or the message for an attachment.
    pub task: String,
    /// Catalog key within that owner, not a filesystem path.
    pub key: String,
    pub media_type: Option<String>,
    pub encoding: ArtifactEncoding,
    /// UTF-8 text or one independently base64-encoded byte chunk.
    /// The complete JSON-RPC reply remains within APP_CONTROL_MAX.
    pub content: String,
    /// More source content exists; continue with DetailReply.next.
    pub truncated: bool,
}

/// A task-text citation that resolves, as the engine resolves it, without its content: the
/// cited artifact's catalog row, and the size of the cited section's source when it names one.
/// One that does not resolve is answered `not_found`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CitationDetail {
    #[schemars(with = "crate::schema_types::Artifact")]
    pub artifact: Artifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactsPage {
    #[schemars(with = "Vec<crate::schema_types::Artifact>")]
    pub rows: Vec<Artifact>,
}

/// All supplied filters must match. Prefixes are case-insensitive.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactNarrowing {
    /// Include submission, record, and replaced artifacts without a kind filter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEncoding {
    Text,
    Base64,
    #[serde(other)]
    Other,
}
