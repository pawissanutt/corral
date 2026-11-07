//! Native billing-route identity. A key or a connection binding never authorizes spending.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionId {
    AnthropicSubscription,
    AnthropicApi,
    OpenaiSubscription,
    OpenaiApi,
    AmazonBedrock,
    #[serde(rename = "llama.cpp")]
    LlamaCpp,
}

impl ConnectionId {
    pub fn id(self) -> &'static str {
        match self {
            Self::AnthropicSubscription => "anthropic-subscription",
            Self::AnthropicApi => "anthropic-api",
            Self::OpenaiSubscription => "openai-subscription",
            Self::OpenaiApi => "openai-api",
            Self::AmazonBedrock => "amazon-bedrock",
            Self::LlamaCpp => "llama.cpp",
        }
    }

    pub fn vendor(self) -> &'static str {
        match self {
            Self::AnthropicSubscription | Self::AnthropicApi => "anthropic",
            Self::OpenaiSubscription | Self::OpenaiApi => "openai",
            Self::AmazonBedrock => "amazon-bedrock",
            Self::LlamaCpp => "llama.cpp",
        }
    }

    pub fn provider(self) -> &'static str {
        match self {
            Self::OpenaiSubscription => "openai-codex",
            Self::OpenaiApi => "openai",
            _ => self.vendor(),
        }
    }

    pub fn api_key(self) -> bool {
        matches!(self, Self::AnthropicApi | Self::OpenaiApi)
    }

    pub fn legacy(vendor: &str) -> Option<Self> {
        match vendor {
            "anthropic" => Some(Self::AnthropicSubscription),
            "openai" => Some(Self::OpenaiSubscription),
            "amazon-bedrock" => Some(Self::AmazonBedrock),
            "llama.cpp" => Some(Self::LlamaCpp),
            _ => None,
        }
    }

    pub fn parse(id: &str) -> Result<Self> {
        serde_json::from_value(serde_json::Value::String(id.to_owned()))
            .map_err(|_| anyhow::anyhow!("connection_invalid: unknown native connection '{id}'"))
    }

    pub fn resolve(selected: Option<Self>, vendor: Option<&str>) -> Result<Self> {
        let connection = selected.or_else(|| vendor.and_then(Self::legacy))
            .ok_or_else(|| anyhow::anyhow!("connection_invalid: no native vendor binding"))?;
        ensure!(Some(connection.vendor()) == vendor, "connection_invalid: connection/vendor mismatch");
        Ok(connection)
    }

    /// First member of T-888: canonical PaidEpisode admission has not landed yet.
    pub fn require_admission(self) -> Result<()> {
        ensure!(!self.api_key(), "paid_admission_required: API connections are disabled until canonical metered admission is installed");
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionBinding {
    pub version: u32,
    pub connection: ConnectionId,
    pub candidate: Option<String>,
    pub model: String,
    pub effort: Option<String>,
    pub digest: String,
}

impl ConnectionBinding {
    pub fn new(connection: ConnectionId, candidate: Option<String>, model: String, effort: Option<String>) -> Result<Self> {
        let bytes = serde_json::to_vec(&(1, connection, &candidate, &model, &effort))?;
        Ok(Self { version: 1, connection, candidate, model, effort, digest: format!("{:x}", Sha256::digest(bytes)) })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1 && *self == Self::new(self.connection, self.candidate.clone(), self.model.clone(), self.effort.clone())?,
            "connection_invalid: stale or malformed binding digest");
        Ok(())
    }
}

impl crate::ResolvedProfile {
    pub fn connection_binding(&self) -> Result<Option<ConnectionBinding>> {
        if !crate::is_corral_pi(&self.harness) {
            ensure!(self.connection.is_none(), "connection_invalid: explicit native connection on another harness");
            return Ok(None);
        }
        let connection = ConnectionId::resolve(self.connection, self.vendor.as_deref())?;
        Ok(Some(ConnectionBinding::new(connection, self.candidate.clone(), self.model.clone().unwrap_or_default(), self.effort.clone())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_binding_preserves_billing_identity_and_fails_closed() {
        let legacy = ConnectionId::resolve(None, Some("openai")).unwrap();
        assert_eq!(legacy.provider(), "openai-codex");
        assert!(legacy.require_admission().is_ok());
        for id in [ConnectionId::OpenaiApi, ConnectionId::AnthropicApi] {
            assert!(id.require_admission().is_err());
            let bound = ConnectionBinding::new(id, Some("exact".into()), "same-model".into(), Some("high".into())).unwrap();
            bound.validate().unwrap();
            let mut stale = bound.clone();
            stale.candidate = Some("another".into());
            assert!(stale.validate().is_err());
        }
        assert!(ConnectionId::parse("proxy").is_err());
        assert!(ConnectionId::resolve(Some(ConnectionId::OpenaiApi), Some("anthropic")).is_err());
    }
}
