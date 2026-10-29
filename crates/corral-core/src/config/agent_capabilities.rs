//! `[agent.capabilities]`: preset plus per-role overrides, resolved against the shared catalog
//! in `agent::capabilities` into one snapshot per generation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::agent::{
    AgentRole, CATALOG, CapabilitySnapshot, CapabilitySource, CapabilityState, SnapshotPreset,
    WebSettings,
};

/// `[agent]`: Pi-native host settings. Every value applies only to pi-native runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentCfg {
    /// Minimal per-role optional-tool settings, resolved once for each generation.
    pub capabilities: AgentCapabilities,
    /// Explicit fixture or development asset installation; never inferred.
    pub assets_dir: Option<PathBuf>,
    /// A waiting question with no PM triage reaches Needs me after this many seconds.
    pub question_unattended_secs: u64,
    /// An uncertain gate start becomes Needs me after this many seconds.
    pub gate_start_uncertain_secs: u64,
    /// Grace for a host to drain before it is fenced.
    pub drain_secs: u64,
    /// Pane look: `rich` (corral chrome) or `plain` Pi.
    pub ui: AgentUi,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentUi {
    #[default]
    Rich,
    Plain,
}

impl AgentUi {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentUi::Rich => "rich",
            AgentUi::Plain => "plain",
        }
    }
}

impl Default for AgentCfg {
    fn default() -> Self {
        Self {
            capabilities: AgentCapabilities::default(),
            assets_dir: None,
            question_unattended_secs: 600,
            gate_start_uncertain_secs: 300,
            drain_secs: 60,
            ui: AgentUi::Rich,
        }
    }
}

impl AgentCfg {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Strict,
    #[default]
    Standard,
    Full,
}

impl Preset {
    fn index(self) -> usize {
        match self {
            Preset::Strict => 0,
            Preset::Standard => 1,
            Preset::Full => 2,
        }
    }

    fn snapshot(self) -> SnapshotPreset {
        match self {
            Preset::Strict => SnapshotPreset::Strict,
            Preset::Standard => SnapshotPreset::Standard,
            Preset::Full => SnapshotPreset::Full,
        }
    }
}

/// `[agent.capabilities.<role>.web]`: optional preset override and GET-only web policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub settings: WebSettings,
}

/// `[agent.capabilities.<role>]`: boolean overrides plus compound web settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<WebCapabilities>,
    #[serde(flatten)]
    overrides: BTreeMap<String, bool>,
}

impl RoleCapabilities {
    pub fn get(&self, key: &str) -> Option<bool> {
        if key == "web.enabled" {
            self.web
                .as_ref()
                .and_then(|web| web.enabled)
                .or_else(|| self.overrides.get(key).copied())
        } else {
            self.overrides.get(key).copied()
        }
    }

    pub fn set(&mut self, key: &str, enabled: bool) {
        if key == "web.enabled" {
            self.overrides.remove(key);
            self.web.get_or_insert_default().enabled = Some(enabled);
        } else {
            self.overrides.insert(key.to_owned(), enabled);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty() && self.web.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RawCapabilities {
    pub preset: Preset,
    #[serde(skip_serializing_if = "RoleCapabilities::is_empty")]
    pub worker: RoleCapabilities,
    #[serde(skip_serializing_if = "RoleCapabilities::is_empty")]
    pub reviewer: RoleCapabilities,
    #[serde(skip_serializing_if = "RoleCapabilities::is_empty")]
    pub pm: RoleCapabilities,
    #[serde(skip_serializing_if = "RoleCapabilities::is_empty")]
    pub planner: RoleCapabilities,
    #[serde(skip_serializing_if = "RoleCapabilities::is_empty")]
    pub plan_reviewer: RoleCapabilities,
}

/// Validated `[agent.capabilities]`: every override names a catalog key its role may use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCapabilities", into = "RawCapabilities")]
pub struct AgentCapabilities(RawCapabilities);

impl TryFrom<RawCapabilities> for AgentCapabilities {
    type Error = String;

    fn try_from(raw: RawCapabilities) -> Result<Self, String> {
        let config = Self(raw);
        for role in ROLES {
            let settings = config.role(role);
            if let Some(web) = &settings.web {
                if web.enabled.is_some() && settings.overrides.contains_key("web.enabled") {
                    return Err("web.enabled is configured both as a key and a web setting".into());
                }
                validate_web(&web.settings)?;
            }
            for key in settings
                .overrides
                .keys()
                .map(String::as_str)
                .chain(settings.web.as_ref().map(|_| "web.enabled"))
            {
                let def = CATALOG.iter().find(|def| def.key == key).ok_or_else(|| {
                    let known: Vec<&str> = CATALOG.iter().map(|def| def.key).collect();
                    format!(
                        "[agent.capabilities.{}] {key} is not a known capability (known: {})",
                        role.as_str(),
                        known.join(", ")
                    )
                })?;
                if (def.defaults)(role).is_none() {
                    return Err(format!(
                        "[agent.capabilities.{}] {key} is not available to the {} role",
                        role.as_str(),
                        role.as_str()
                    ));
                }
            }
        }
        Ok(config)
    }
}

impl From<AgentCapabilities> for RawCapabilities {
    fn from(config: AgentCapabilities) -> Self {
        config.0
    }
}

const ROLES: [AgentRole; 5] = [
    AgentRole::Worker,
    AgentRole::Reviewer,
    AgentRole::Pm,
    AgentRole::Planner,
    AgentRole::PlanReviewer,
];

impl AgentCapabilities {
    pub fn preset(&self) -> Preset {
        self.0.preset
    }

    pub fn set_preset(&mut self, preset: Preset) {
        self.0.preset = preset;
    }

    fn role(&self, role: AgentRole) -> &RoleCapabilities {
        match role {
            AgentRole::Worker => &self.0.worker,
            AgentRole::Reviewer => &self.0.reviewer,
            AgentRole::Pm => &self.0.pm,
            AgentRole::Planner => &self.0.planner,
            AgentRole::PlanReviewer => &self.0.plan_reviewer,
        }
    }

    /// Sets a role override after checking it against the catalog, as parsing does.
    pub fn set(&mut self, role: AgentRole, key: &str, enabled: bool) -> Result<(), String> {
        let mut raw = self.0.clone();
        match role {
            AgentRole::Worker => raw.worker.set(key, enabled),
            AgentRole::Reviewer => raw.reviewer.set(key, enabled),
            AgentRole::Pm => raw.pm.set(key, enabled),
            AgentRole::Planner => raw.planner.set(key, enabled),
            AgentRole::PlanReviewer => raw.plan_reviewer.set(key, enabled),
        }
        *self = Self::try_from(raw)?;
        Ok(())
    }

    /// The effective snapshot for `role`. `supported` is the installed host's implemented keys;
    /// `None` skips that check. A requested key the host lacks is reported and left off.
    pub fn resolve(
        &self,
        role: AgentRole,
        supported: Option<&BTreeSet<String>>,
    ) -> CapabilitySnapshot {
        let overrides = self.role(role);
        let features = CATALOG
            .iter()
            .filter_map(|def| {
                let defaults = (def.defaults)(role)?;
                let (wanted, source) = match overrides.get(def.key) {
                    Some(value) => (value, CapabilitySource::RoleOverride),
                    None => (defaults[self.0.preset.index()], CapabilitySource::Preset),
                };
                let unavailable = (wanted && supported.is_some_and(|keys| !keys.contains(def.key)))
                    .then(|| {
                        format!(
                            "the installed Pi host does not implement {}; run `corral agent install` \
                             to update it",
                            def.key
                        )
                    });
                let state = CapabilityState {
                    enabled: wanted && unavailable.is_none(),
                    source,
                    unavailable,
                };
                Some((def.key.to_owned(), state))
            })
            .collect();
        let web = overrides
            .web
            .as_ref()
            .map_or_else(WebSettings::default, |web| web.settings.clone());
        CapabilitySnapshot::with_web(role, self.0.preset.snapshot(), features, Some(web))
    }

    pub fn todo(&self, role: AgentRole) -> bool {
        self.resolve(role, None).enabled("todo")
    }
}

fn validate_web(web: &WebSettings) -> Result<(), String> {
    if web.max_bytes == 0 || web.max_bytes > 9_007_199_254_740_991 {
        return Err("web.max_bytes must be a positive JavaScript safe integer".into());
    }
    if web.timeout_ms == 0 || web.timeout_ms > 2_147_483_647 {
        return Err("web.timeout_ms must be between 1 and 2147483647".into());
    }
    if web.max_redirects > 10 {
        return Err("web.max_redirects must be between 0 and 10".into());
    }
    if let Some(url) = &web.searxng_url {
        let endpoint = regex::Regex::new(
            r"^https?://([A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?|\[[0-9A-Fa-f:]+\])(?::([0-9]+))?(?:[/?][^\s#\\]*)?$",
        ).map_err(|error| format!("compiling web endpoint pattern: {error}"))?;
        let captures = endpoint
            .captures(url)
            .ok_or("web.searxng_url must be an HTTP(S) endpoint without userinfo or fragments")?;
        if let Some(port) = captures.get(2)
            && port.as_str().parse::<u16>().is_err()
        {
            return Err("web.searxng_url has an invalid port".into());
        }
        if let Some(host) = captures.get(1)
            && host.as_str().starts_with('[')
            && host.as_str()[1..host.as_str().len() - 1]
                .parse::<std::net::Ipv6Addr>()
                .is_err()
        {
            return Err("web.searxng_url has an invalid IPv6 host".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_overrides_respect_role_ceiling() {
        // Given each preset and a per-role override different from its default.
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            let mut config = AgentCapabilities::default();
            config.set_preset(preset);
            let enabled = preset != Preset::Strict;
            assert_eq!(config.todo(AgentRole::Worker), enabled);
            assert_eq!(config.todo(AgentRole::Pm), enabled);
            // When worker overrides its preset.
            config.set(AgentRole::Worker, "todo", !enabled).unwrap();
            // Then PM retains its default and foreign roles remain excluded.
            assert_eq!(config.todo(AgentRole::Worker), !enabled);
            assert_eq!(config.todo(AgentRole::Pm), enabled);
            for role in [
                AgentRole::Reviewer,
                AgentRole::Planner,
                AgentRole::PlanReviewer,
            ] {
                assert!(!config.todo(role));
            }
        }
    }

    #[test]
    fn omitted_capabilities_default_to_standard_and_parse_role_disable() {
        // Given absent configuration and a configured worker opt-out.
        let absent: crate::config::AgentCfg = toml::from_str("").unwrap();
        let configured: crate::config::AgentCfg =
            toml::from_str("[capabilities.worker]\ntodo = false").unwrap();
        // Then absent config enables worker/PM, while only the worker opts out.
        assert!(absent.capabilities.todo(AgentRole::Worker));
        assert!(absent.capabilities.todo(AgentRole::Pm));
        assert!(!configured.capabilities.todo(AgentRole::Worker));
        assert!(configured.capabilities.todo(AgentRole::Pm));
    }

    #[test]
    fn overrides_outside_the_catalog_or_role_ceiling_are_config_errors() {
        // Given an unknown key, a ceiling-disallowed key and every role table.
        let parse = |text: &str| toml::from_str::<crate::config::AgentCfg>(text).map(|_| ());
        // Then parsing names the table and the reason; allowed tables parse.
        let unknown = parse("[capabilities.worker]\nteleport = true").unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("teleport is not a known capability")
        );
        let ceiling = parse("[capabilities.reviewer]\ntodo = true").unwrap_err();
        assert!(
            ceiling
                .to_string()
                .contains("not available to the reviewer role")
        );
        parse("[capabilities]\npreset = \"full\"\n[capabilities.planner]\n[capabilities.plan_reviewer]")
            .unwrap();
        let mut config = AgentCapabilities::default();
        assert!(config.set(AgentRole::Planner, "todo", true).is_err());
        assert_eq!(config, AgentCapabilities::default());
    }

    #[test]
    fn snapshot_records_values_sources_and_a_stable_digest() {
        // Given the default config and a worker override.
        let standard = AgentCapabilities::default();
        let mut overridden = standard.clone();
        overridden.set(AgentRole::Worker, "todo", false).unwrap();
        // When each resolves the worker snapshot.
        let a = standard.resolve(AgentRole::Worker, None);
        let b = overridden.resolve(AgentRole::Worker, None);
        // Then values, sources and the digest follow the settings deterministically.
        assert_eq!(a, standard.resolve(AgentRole::Worker, None));
        assert_eq!(a.preset, SnapshotPreset::Standard);
        assert_eq!(a.features["todo"].source, CapabilitySource::Preset);
        assert!(a.enabled("todo"));
        assert_eq!(b.features["todo"].source, CapabilitySource::RoleOverride);
        assert!(!b.enabled("todo"));
        assert_ne!(a.digest, b.digest);
        assert!(
            standard
                .resolve(AgentRole::Reviewer, None)
                .enabled("web.enabled")
        );
        // And the round trip through TOML preserves the overrides.
        let text = toml::to_string(&overridden).unwrap();
        assert_eq!(
            toml::from_str::<AgentCapabilities>(&text).unwrap(),
            overridden
        );
    }

    #[test]
    fn an_older_host_reports_unavailable_defaults_and_names_explicit_requests() {
        // Given an installed host that implements no catalog key.
        let older = BTreeSet::new();
        let mut explicit = AgentCapabilities::default();
        explicit.set(AgentRole::Pm, "todo", true).unwrap();
        // When resolving a preset default and an explicit request.
        let default = AgentCapabilities::default().resolve(AgentRole::Worker, Some(&older));
        let requested = explicit.resolve(AgentRole::Pm, Some(&older));
        // Then both are omitted and reported; only the explicit request is named for refusal.
        assert!(!default.enabled("todo"));
        assert!(default.features["todo"].unavailable.is_some());
        assert_eq!(default.explicit_unavailable(), None);
        assert!(!requested.enabled("todo"));
        assert_eq!(
            requested.explicit_unavailable().map(|(key, _)| key),
            Some("todo")
        );
        let current: BTreeSet<String> = ["todo".to_owned()].into();
        assert!(
            explicit
                .resolve(AgentRole::Pm, Some(&current))
                .enabled("todo")
        );
    }

    #[test]
    fn legacy_and_changed_settings_produce_an_explicit_transition() {
        // Given a legacy generation with todo and a current strict resolution.
        let legacy = CapabilitySnapshot::legacy(AgentRole::Worker, true);
        let mut strict = AgentCapabilities::default();
        strict.set_preset(Preset::Strict);
        let current = strict.resolve(AgentRole::Worker, None);
        // When the next generation compares itself with the old one.
        let transition = current.transition_from(3, &legacy).unwrap();
        // Then the legacy label and the concrete change are recorded, and identity is silent.
        assert_eq!(legacy.preset, SnapshotPreset::LegacyStrict);
        assert!(legacy.enabled("todo"));
        assert_eq!(transition.from_generation, 3);
        assert_eq!(transition.from_preset, SnapshotPreset::LegacyStrict);
        assert_eq!(transition.from_digest, legacy.digest);
        assert_eq!(
            transition.changes,
            vec![crate::agent::CapabilityChange {
                key: "todo".into(),
                from: Some(true),
                to: Some(false),
            }]
        );
        assert!(current.transition_from(4, &current).is_none());
    }

    #[test]
    fn web_presets_overrides_and_role_ceilings_share_the_catalog() {
        // Given every preset and role.
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            let mut config = AgentCapabilities::default();
            config.set_preset(preset);
            for role in [
                AgentRole::Worker,
                AgentRole::Reviewer,
                AgentRole::Planner,
                AgentRole::PlanReviewer,
            ] {
                let snapshot = config.resolve(role, None);
                assert_eq!(snapshot.enabled("web.enabled"), preset != Preset::Strict);
                assert_eq!(
                    snapshot.web,
                    (preset != Preset::Strict).then(WebSettings::default)
                );
                // When web overrides the preset through the existing setter.
                config
                    .set(role, "web.enabled", preset == Preset::Strict)
                    .unwrap();
                // Then the override is honored without changing the role ceiling.
                assert_eq!(
                    config.resolve(role, None).enabled("web.enabled"),
                    preset == Preset::Strict
                );
            }
            assert!(config.set(AgentRole::Pm, "web.enabled", true).is_err());
            assert!(config.resolve(AgentRole::Pm, None).web.is_none());
        }
        assert!(toml::from_str::<AgentCapabilities>("[pm.web]\nhosts = ['*']").is_err());
    }

    #[test]
    fn compound_web_config_round_trips_with_todo_overrides_and_endpoint() {
        // Given a strict preset with independent todo and compound web configuration.
        let text = "preset = 'strict'\n[worker]\ntodo = true\n[worker.web]\n\
                    enabled = true\nhosts = ['docs.example.org']\n\
                    searxng_url = 'http://localhost:8080/search'";
        // When parsed and serialized through the public configuration.
        let config: AgentCapabilities = toml::from_str(text).unwrap();
        let encoded = toml::to_string(&config).unwrap();
        // Then both forms preserve overrides, policy defaults, and the localhost endpoint.
        assert_eq!(
            toml::from_str::<AgentCapabilities>(&encoded).unwrap(),
            config
        );
        let snapshot = config.resolve(AgentRole::Worker, None);
        assert!(snapshot.enabled("todo") && snapshot.enabled("web.enabled"));
        let web = snapshot.web.unwrap();
        assert_eq!(web.hosts, ["docs.example.org"]);
        assert_eq!(web.max_bytes, 1_048_576);
        assert_eq!(web.timeout_ms, 15_000);
        assert_eq!(web.max_redirects, 5);
        assert_eq!(
            web.searxng_url.as_deref(),
            Some("http://localhost:8080/search")
        );
    }

    #[test]
    fn web_config_rejects_invalid_limits_and_endpoints() {
        // Given invalid policy values at the parsing boundary.
        for value in [
            "max_bytes = 0",
            "max_bytes = 9007199254740992",
            "timeout_ms = 0",
            "timeout_ms = 2147483648",
            "max_redirects = -1",
            "max_redirects = 11",
            "searxng_url = 'ftp://localhost/search'",
            "searxng_url = 'https://user:pass@example.org/search'",
            "searxng_url = 'https://example.org/search#fragment'",
            "searxng_url = 'http://localhost:99999'",
            "unknown = true",
        ] {
            // When a compound configuration is parsed, then invalid settings are refused.
            assert!(
                toml::from_str::<AgentCapabilities>(&format!("[worker.web]\n{value}")).is_err(),
                "{value}"
            );
        }
        for url in [
            "http://localhost:8080",
            "https://search.example.org/search",
            "http://[::1]:8080/search",
        ] {
            assert!(
                toml::from_str::<AgentCapabilities>(&format!(
                    "[worker.web]\nsearxng_url = '{url}'"
                ))
                .is_ok(),
                "{url}"
            );
        }
    }

    #[test]
    fn host_and_endpoint_settings_each_change_the_snapshot_digest() {
        // Given a standard worker policy and independent host and endpoint changes.
        let original = AgentCapabilities::default().resolve(AgentRole::Worker, None);
        for setting in [
            "hosts = ['docs.example.org']",
            "searxng_url = 'http://localhost:8080'",
        ] {
            let config: AgentCapabilities =
                toml::from_str(&format!("[worker.web]\n{setting}")).unwrap();
            // When resolving settings, then the deterministic digest and concrete diff change.
            let snapshot = config.resolve(AgentRole::Worker, None);
            assert_eq!(snapshot, config.resolve(AgentRole::Worker, None));
            assert_ne!(snapshot.digest, original.digest);
            let transition = snapshot.transition_from(1, &original).unwrap();
            assert!(transition.changes.is_empty());
            assert!(transition.web_change.is_some());
        }
    }

    #[test]
    fn bool_map_web_override_preserves_public_get_and_set_behavior() {
        // Given the existing boolean-map form with a dotted catalog key.
        let mut role: RoleCapabilities =
            toml::from_str("\"web.enabled\" = false\ntodo = true").unwrap();
        assert_eq!(role.get("web.enabled"), Some(false));
        // When the public setter replaces that override.
        role.set("web.enabled", true);
        // Then both getters and the serialized compound form retain the effective values.
        assert_eq!(role.get("web.enabled"), Some(true));
        assert_eq!(role.get("todo"), Some(true));
        let encoded = toml::to_string(&role).unwrap();
        let decoded: RoleCapabilities = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, role);
    }

    #[test]
    fn web_policy_accepts_transport_boundaries_and_independent_search_endpoint() {
        // Given the exact transport bounds and an empty fetch allowlist.
        let text = "[worker.web]\nhosts = []\nmax_bytes = 9007199254740991\n\
                    timeout_ms = 2147483647\nmax_redirects = 10\n\
                    searxng_url = 'http://localhost:8080'";
        // When policy resolves, then endpoint configuration does not broaden fetch hosts.
        let config: AgentCapabilities = toml::from_str(text).unwrap();
        let settings = config.resolve(AgentRole::Worker, None).web.unwrap();
        assert!(settings.hosts.is_empty());
        assert_eq!(settings.max_bytes, 9_007_199_254_740_991);
        assert_eq!(settings.timeout_ms, 2_147_483_647);
        assert_eq!(settings.max_redirects, 10);
        assert_eq!(
            settings.searxng_url.as_deref(),
            Some("http://localhost:8080")
        );
    }
}
