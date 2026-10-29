//! Wave-0 optional capabilities. Later catalog entries extend this shared resolver.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::agent::AgentRole;

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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoleCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentCapabilities {
    pub preset: Preset,
    pub worker: RoleCapabilities,
    pub pm: RoleCapabilities,
}

impl AgentCapabilities {
    /// A preset never expands a role's authority; todo belongs only to worker and PM.
    pub fn todo(&self, role: AgentRole) -> bool {
        let overrides = match role {
            AgentRole::Worker => &self.worker,
            AgentRole::Pm => &self.pm,
            AgentRole::Reviewer | AgentRole::Planner | AgentRole::PlanReviewer => return false,
        };
        overrides.todo.unwrap_or(self.preset != Preset::Strict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_overrides_respect_role_ceiling() {
        // Given each preset and a per-role override different from its default.
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            let mut config = AgentCapabilities {
                preset,
                ..Default::default()
            };
            let enabled = preset != Preset::Strict;
            assert_eq!(config.todo(AgentRole::Worker), enabled);
            assert_eq!(config.todo(AgentRole::Pm), enabled);
            // When worker overrides its preset.
            config.worker.todo = Some(!enabled);
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
}
