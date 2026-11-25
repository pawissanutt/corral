//! The owner reads and replaces the TOML document; the daemon validates it before writing.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONFIG_GET: &str = "config.get";
pub const CONFIG_PUT: &str = "config.put";
pub const PI_STARTUP_V1: &str = "config.pi_startup.v1";
pub const PI_STARTUP: &str = "config.pi_startup";
pub const PI_HOSTED_TOOLS_V1: &str = "config.pi_hosted_tools.v1";
pub const PI_HOSTED_TOOLS: &str = "config.pi_hosted_tools";

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

/// Read or replace both hosted tool overrides without exposing raw Pi TOML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiHostedToolsRequest {
    pub change: Option<PiHostedToolsChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiHostedToolsChange {
    pub revision: String,
    /// Null restores legacy web search selection.
    pub web_search: Option<String>,
    /// Null removes the override (image generation defaults to disabled).
    pub image_gen: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiHostedToolsConfig {
    pub revision: String,
    pub web_search: Option<String>,
    pub image_gen: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn pi_hosted_tools_wire_distinguishes_null_and_off() {
        let read = json!({"change": null});
        let request = serde_json::from_value::<PiHostedToolsRequest>(read.clone()).unwrap();
        assert!(request.change.is_none());
        assert_eq!(serde_json::to_value(request).unwrap(), read);

        let update = json!({"revision": "current", "web_search": null, "image_gen": "off"});
        let request_json = json!({"change": update.clone()});
        let request = serde_json::from_value::<PiHostedToolsRequest>(request_json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), request_json);
        let change = request.change.unwrap();
        assert_eq!(change.web_search, None);
        assert_eq!(change.image_gen.as_deref(), Some("off"));
        let response = PiHostedToolsConfig {
            revision: change.revision,
            web_search: change.web_search,
            image_gen: change.image_gen,
        };
        assert_eq!(serde_json::to_value(response).unwrap(), update);

        let mut extra = update;
        extra["unexpected"] = Value::Null;
        assert!(serde_json::from_value::<PiHostedToolsRequest>(json!({"change": extra})).is_err());
    }
}
