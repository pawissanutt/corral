//! `[agent.capabilities]`: preset plus per-role overrides, resolved against the shared catalog
//! in `agent::capabilities` into one snapshot per generation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::agent::{
    AgentRole, CATALOG, CapabilitySnapshot, CapabilitySource, CapabilityState, SnapshotPreset,
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

/// `[agent.capabilities.<role>]`: catalog key to boolean override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleCapabilities(BTreeMap<String, bool>);

impl RoleCapabilities {
    pub fn get(&self, key: &str) -> Option<bool> {
        self.0.get(key).copied()
    }

    pub fn set(&mut self, key: &str, enabled: bool) {
        self.0.insert(key.to_owned(), enabled);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
            for key in config.role(role).0.keys() {
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
        CapabilitySnapshot::new(role, self.0.preset.snapshot(), features)
    }

    pub fn todo(&self, role: AgentRole) -> bool {
        self.resolve(role, None).enabled("todo")
    }
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
                .features
                .is_empty()
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
}
