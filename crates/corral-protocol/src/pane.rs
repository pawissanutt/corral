//! Herdr pane controls for sessions attached to engine runs.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PANE_FOCUS: &str = "pane.focus";
pub const PANE_KEYS: &str = "pane.keys";

/// Resolve the pane from the stored run rather than taking a runner ID from the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PaneFocus {
    pub task: String,
    pub run: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PaneKeys {
    #[serde(flatten)]
    pub target: PaneFocus,
    pub keys: Vec<String>,
}
