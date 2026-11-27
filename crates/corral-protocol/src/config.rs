//! The owner reads and replaces the TOML document; the daemon validates it before writing.
mod skills;
pub use skills::*;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONFIG_GET: &str = "config.get";
pub const CONFIG_PUT: &str = "config.put";
pub const PI_STARTUP_V1: &str = "config.pi_startup.v1";
pub const PI_STARTUP: &str = "config.pi_startup";
pub const PI_HOSTED_TOOLS_V1: &str = "config.pi_hosted_tools.v1";
pub const PI_HOSTED_TOOLS: &str = "config.pi_hosted_tools";
pub const PI_HOSTED_TOOL_MODELS_V1: &str = "config.pi_hosted_tool_models.v1";
pub const PI_HOSTED_TOOL_MODELS: &str = "config.pi_hosted_tool_models";

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
#[schemars(extend("required" = ["revision", "web_search", "image_gen"]))]
pub struct PiHostedToolsChange {
    pub revision: String,
    /// Null restores legacy web search selection; omission is not a clear request.
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub web_search: Option<String>,
    /// Null removes the override (image generation defaults to disabled).
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub image_gen: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PiHostedToolsConfig {
    pub revision: String,
    pub web_search: Option<String>,
    pub image_gen: Option<String>,
}

/// Read the selected provider's model metadata without changing Pi preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiHostedToolModelsRequest {
    pub tool: PiHostedTool,
    pub provider: PiHostedToolProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PiHostedTool {
    WebSearch,
    ImageGen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PiHostedToolProvider {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "openai-codex")]
    OpenAiCodex,
    Anthropic,
    AnthropicApi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PiHostedToolModels {
    pub status: PiHostedToolModelsStatus,
    /// Provider model IDs, not qualified selectors. Empty on failure or no matches.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PiHostedToolModelsStatus {
    Live,
    Unsupported,
    MissingLogin,
    Failed,
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

        for field in ["web_search", "image_gen"] {
            let mut partial = update.clone();
            partial.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<PiHostedToolsRequest>(json!({"change": partial})).is_err()
            );
        }
        let mut extra = update;
        extra["unexpected"] = Value::Null;
        assert!(serde_json::from_value::<PiHostedToolsRequest>(json!({"change": extra})).is_err());
    }
}
