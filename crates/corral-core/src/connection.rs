//! Native billing-route identity. A key or a connection binding never authorizes spending.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingMode {
    Subscription,
    Metered,
    Local,
}

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

    pub fn billing(self) -> BillingMode {
        match self {
            Self::AnthropicSubscription | Self::OpenaiSubscription => BillingMode::Subscription,
            Self::LlamaCpp => BillingMode::Local,
            _ => BillingMode::Metered,
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
        let connection = selected
            .or_else(|| vendor.and_then(Self::legacy))
            .ok_or_else(|| anyhow::anyhow!("connection_invalid: no native vendor binding"))?;
        ensure!(
            Some(connection.vendor()) == vendor,
            "connection_invalid: connection/vendor mismatch"
        );
        Ok(connection)
    }

    /// First member of T-888: canonical PaidEpisode admission has not landed yet.
    pub fn require_admission(self) -> Result<()> {
        ensure!(
            !self.api_key(),
            "paid_admission_required: API connections are disabled until canonical metered admission is installed"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionOrigin {
    Registered,
    LegacySubscription,
    LegacyBedrock,
    LegacyLocal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionBinding {
    pub version: u32,
    pub connection: ConnectionId,
    /// Original selector/account-group identity, never used as a credential path or provider.
    pub identity: String,
    pub origin: ConnectionOrigin,
    pub candidate: Option<String>,
    pub model: String,
    pub effort: Option<String>,
    pub digest: String,
}

impl ConnectionBinding {
    pub fn new(
        connection: ConnectionId,
        candidate: Option<String>,
        model: String,
        effort: Option<String>,
    ) -> Result<Self> {
        Self::for_identity(
            connection,
            connection.id().to_owned(),
            candidate,
            model,
            effort,
        )
    }

    fn for_identity(
        connection: ConnectionId,
        identity: String,
        candidate: Option<String>,
        model: String,
        effort: Option<String>,
    ) -> Result<Self> {
        ensure!(
            candidate.as_ref().is_none_or(|c| !c.trim().is_empty()
                && c.len() <= 512
                && !c.chars().any(char::is_control)),
            "connection_invalid: malformed candidate identity"
        );
        ensure!(
            !identity.trim().is_empty()
                && identity.len() <= 256
                && !identity.chars().any(char::is_control),
            "connection_invalid: malformed connection identity"
        );
        let origin = if identity == connection.id() {
            ConnectionOrigin::Registered
        } else {
            ensure!(
                candidate.is_some() && ConnectionId::parse(&identity).is_err(),
                "connection_invalid: unproven legacy identity"
            );
            match connection {
                ConnectionId::AnthropicSubscription | ConnectionId::OpenaiSubscription => {
                    ConnectionOrigin::LegacySubscription
                }
                ConnectionId::AmazonBedrock => ConnectionOrigin::LegacyBedrock,
                ConnectionId::LlamaCpp => ConnectionOrigin::LegacyLocal,
                _ => anyhow::bail!("connection_invalid: API connections cannot use legacy aliases"),
            }
        };
        let bytes = serde_json::to_vec(&(
            1, connection, &identity, &candidate, &model, &effort, origin,
        ))?;
        Ok(Self {
            version: 1,
            connection,
            identity,
            origin,
            candidate,
            model,
            effort,
            digest: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    pub fn continue_from(previous: Option<&Self>, next: Option<&Self>, fresh: bool) -> Result<()> {
        if let Some(previous) = previous {
            previous.validate()?;
        }
        if let Some(next) = next {
            next.validate()?;
        }
        ensure!(
            fresh || previous == next,
            "connection_binding_changed: retained session requires its frozen binding; use an authorized fresh role attempt/session"
        );
        Ok(())
    }

    pub fn requires_ack(&self) -> bool {
        matches!(
            self.origin,
            ConnectionOrigin::Registered | ConnectionOrigin::LegacySubscription
        )
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && *self
                    == Self::for_identity(
                        self.connection,
                        self.identity.clone(),
                        self.candidate.clone(),
                        self.model.clone(),
                        self.effort.clone()
                    )?,
            "connection_invalid: stale or malformed binding digest"
        );
        Ok(())
    }
}

impl crate::ExecutionCandidate {
    /// The old native Anthropic/OpenAI routes were OAuth-only independently of the decorative
    /// account label. Unknown cost remains unknown; this supplies no fallback/cost permission.
    pub(crate) fn native_connection(
        &self,
        explicit_profile: Option<ConnectionId>,
    ) -> Result<ConnectionId> {
        if let Ok(connection) = ConnectionId::parse(&self.connection) {
            ConnectionId::resolve(Some(connection), Some(&self.provider))?;
            ensure!(
                connection.billing() != BillingMode::Subscription
                    || matches!(
                        self.cost,
                        crate::CandidateCost::Unknown | crate::CandidateCost::Subscription
                    ),
                "subscription connection cannot claim metered API spending"
            );
            ensure!(
                !connection.api_key()
                    || matches!(
                        self.cost,
                        crate::CandidateCost::Metered {
                            maximum_micros: 1..,
                            ..
                        }
                    ),
                "API candidate requires a finite positive metered cost"
            );
            return Ok(connection);
        }
        ensure!(
            explicit_profile.is_none(),
            "connection_invalid: unknown explicit candidate connection"
        );
        match (self.provider.as_str(), &self.cost) {
            (
                "anthropic" | "openai",
                crate::CandidateCost::Unknown | crate::CandidateCost::Subscription,
            ) => ConnectionId::legacy(&self.provider)
                .ok_or_else(|| anyhow::anyhow!("connection_invalid: unproven subscription route")),
            // Retain the existing independently bound AWS/local routes, not subscription aliases.
            ("amazon-bedrock", crate::CandidateCost::Metered { .. }) => {
                Ok(ConnectionId::AmazonBedrock)
            }
            ("llama.cpp", crate::CandidateCost::Unknown) => Ok(ConnectionId::LlamaCpp),
            _ => anyhow::bail!(
                "connection_invalid: unknown candidate connection cannot select a metered/API route"
            ),
        }
    }
}

impl crate::ResolvedProfile {
    pub fn connection_binding(&self) -> Result<Option<ConnectionBinding>> {
        if !crate::is_corral_pi(&self.harness) {
            ensure!(
                self.connection.is_none(),
                "connection_invalid: explicit native connection on another harness"
            );
            return Ok(None);
        }
        let inferred = self.model.as_deref().and_then(crate::infer_vendor);
        let connection = ConnectionId::resolve(
            self.connection,
            self.vendor.as_deref().or(inferred.as_deref()),
        )?;
        if self.connection.is_some() {
            ensure!(
                self.model
                    .as_deref()
                    .and_then(crate::infer_vendor)
                    .is_none_or(|vendor| vendor == connection.vendor()),
                "connection_invalid: model family disagrees with connection"
            );
        }
        Ok(Some(ConnectionBinding::for_identity(
            connection,
            self.connection_identity
                .clone()
                .unwrap_or_else(|| connection.id().to_owned()),
            self.candidate.clone(),
            self.model.clone().unwrap_or_default(),
            self.effort.clone(),
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_profiles_and_candidates_preserve_the_selected_connection() {
        let mut config = crate::Config::default_for(std::path::Path::new("/offline-fixture"));
        for (id, model) in [
            (ConnectionId::OpenaiApi, "gpt-5.4"),
            (ConnectionId::AnthropicApi, "claude-sonnet-4-6"),
        ] {
            let profile: crate::Profile = serde_json::from_value(serde_json::json!({
                "harness":"corral-pi", "vendor":id.vendor(), "model":model, "connection":id,
            }))
            .unwrap();
            config.profiles.insert("api".into(), profile);
            let resolved = config
                .resolve_profile(
                    &crate::ProfileChoice {
                        profile: Some("api".into()),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .unwrap();
            let candidate = crate::ExecutionCandidate {
                id: "approved".into(),
                connection: id.id().into(),
                provider: id.vendor().into(),
                harness: "corral-pi".into(),
                model: model.into(),
                effort: None,
                args: Vec::new(),
                evidence: "synthetic".into(),
                cost: crate::CandidateCost::Metered {
                    currency: "USD".into(),
                    maximum_micros: 10,
                },
            };
            let selected = config.resolve_candidate("api", &candidate).unwrap();
            assert_eq!(selected.connection, resolved.connection);
            assert_eq!(selected.candidate.as_deref(), Some("approved"));
            let snapshot = crate::ExecutionSnapshot::new(&config, selected);
            let saved: crate::ExecutionSnapshot =
                serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
            assert_eq!(
                saved.resolved.connection_binding().unwrap(),
                snapshot.resolved.connection_binding().unwrap()
            );
            assert_eq!(id.billing(), BillingMode::Metered);
        }
        assert!(
            serde_json::from_value::<crate::Profile>(
                serde_json::json!({"harness":"corral-pi", "connection":"unknown"})
            )
            .is_err()
        );
    }

    #[test]
    fn native_binding_preserves_billing_identity_and_fails_closed() {
        // Shared Rust/Pi protocol vector: candidate and billing identity are in the digest.
        let vector = ConnectionBinding::new(
            ConnectionId::OpenaiApi,
            Some("exact-api".into()),
            "gpt-5.4".into(),
            None,
        )
        .unwrap();
        assert_eq!(
            vector.digest,
            "c89f600694189d007ba94f7e886d994487cb2e4e6f2822ba1ca60cd2bb9f3e40"
        );
        let legacy = ConnectionId::resolve(None, Some("openai")).unwrap();
        assert_eq!(legacy.provider(), "openai-codex");
        assert!(legacy.require_admission().is_ok());
        for id in [ConnectionId::OpenaiApi, ConnectionId::AnthropicApi] {
            assert!(id.require_admission().is_err());
            let bound = ConnectionBinding::new(
                id,
                Some("exact".into()),
                "same-model".into(),
                Some("high".into()),
            )
            .unwrap();
            bound.validate().unwrap();
            ConnectionBinding::continue_from(Some(&bound), Some(&bound), false).unwrap();
            assert!(ConnectionBinding::continue_from(Some(&bound), None, false).is_err());
            let changed = ConnectionBinding::new(
                id,
                Some("another".into()),
                "same-model".into(),
                Some("high".into()),
            )
            .unwrap();
            assert!(ConnectionBinding::continue_from(Some(&bound), Some(&changed), false).is_err());
            ConnectionBinding::continue_from(Some(&bound), Some(&changed), true).unwrap();
            let mut stale = bound.clone();
            stale.candidate = Some("another".into());
            assert!(stale.validate().is_err());
        }
        assert!(ConnectionId::parse("proxy").is_err());
        assert!(ConnectionId::resolve(Some(ConnectionId::OpenaiApi), Some("anthropic")).is_err());
    }
}
