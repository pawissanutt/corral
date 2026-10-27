//! Harness session policy and the durable record of a role's session selection.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Whether a role may continue its previous harness session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPolicy {
    #[default]
    Reuse,
    Fresh,
}

impl SessionPolicy {
    pub(crate) fn is_reuse(&self) -> bool {
        *self == Self::Reuse
    }
}

/// How the role's harness session was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    New,
    Reused,
    Resumed,
}

/// Session identity and launch metadata captured independently for each role run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSession {
    pub policy: SessionPolicy,
    pub id: Option<String>,
    pub transcript: Option<PathBuf>,
    pub directory: PathBuf,
    pub harness: String,
    /// Frozen CLI flag family, independent of later harness configuration edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub action: SessionAction,
    pub diagnostic: Option<String>,
}
