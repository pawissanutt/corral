//! The owner reads and replaces the TOML document; the daemon validates it before writing.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONFIG_GET: &str = "config.get";
pub const CONFIG_PUT: &str = "config.put";
pub const PI_STARTUP_V1: &str = "config.pi_startup.v1";
pub const PI_STARTUP: &str = "config.pi_startup";

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

/// Read the startup setting, or update the single key without exposing raw Pi TOML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiStartupRequest {
    pub change: Option<PiStartupChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiStartupChange {
    pub revision: String,
    /// Null clears the workspace override.
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiStartupConfig {
    pub revision: String,
    pub model: Option<String>,
}
