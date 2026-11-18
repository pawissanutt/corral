//! The owner reads and replaces the TOML document; the daemon validates it before writing.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONFIG_GET: &str = "config.get";
pub const CONFIG_PUT: &str = "config.put";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigGet {}

/// The `config.get` result and `config.put` parameters. Raw TOML preserves comments and formatting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigDocument {
    pub toml: String,
    /// Content fingerprint of the returned file; not a write precondition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}
