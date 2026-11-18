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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactsPage {
    #[schemars(with = "Vec<crate::schema_types::Artifact>")]
    pub rows: Vec<Artifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEncoding {
    Text,
    Base64,
    #[serde(other)]
    Other,
}
