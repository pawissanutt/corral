//! Generation-bound explore settings and validation of retired oracle configuration.
use super::CapabilitySettings;
use serde::{Deserialize, Serialize};

pub(crate) use super::capabilities::pm_aux::{expert_settings, shadow_settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExploreServiceTier {
    Default,
    Priority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExploreSettings {
    pub max_parallel: u32,
    pub model: Option<String>,
    pub max_turns: u32,
    pub timeout_secs: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ExploreServiceTier>,
}
impl Default for ExploreSettings {
    fn default() -> Self {
        Self {
            max_parallel: 32,
            model: None,
            max_turns: 12,
            timeout_secs: 120,
            service_tier: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FleetSettings {
    pub max_parallel: u32,
    pub max_turns: u32,
    pub timeout_secs: u32,
}
impl Default for FleetSettings {
    fn default() -> Self {
        Self {
            max_parallel: 32,
            max_turns: 60,
            timeout_secs: 1800,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OracleSettings {
    pub model: Option<String>,
    pub timeout_secs: u32,
}
impl Default for OracleSettings {
    fn default() -> Self {
        Self {
            model: None,
            timeout_secs: 120,
        }
    }
}

pub(super) fn model_ok(model: Option<&str>) -> Result<(), String> {
    if model
        .is_some_and(|s| s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err("auxiliary model must be 1-256 non-control characters".into());
    }
    Ok(())
}
pub(super) fn timeout_ok(timeout: u32) -> Result<(), String> {
    if !(1..=600).contains(&timeout) {
        return Err("auxiliary timeout_secs must be 1-600".into());
    }
    Ok(())
}
pub(crate) fn explore_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: ExploreSettings =
        serde_json::from_value(serde_json::json!(config)).map_err(|e| e.to_string())?;
    if !(1..=32).contains(&settings.max_parallel) || !(1..=64).contains(&settings.max_turns) {
        return Err("subagents needs max_parallel=1-32 and max_turns=1-64".into());
    }
    model_ok(settings.model.as_deref())?;
    timeout_ok(settings.timeout_secs)?;
    serde_json::from_value(serde_json::json!(settings)).map_err(|e| e.to_string())
}
pub(crate) fn fleet_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: FleetSettings =
        serde_json::from_value(serde_json::json!(config)).map_err(|e| e.to_string())?;
    if !(1..=32).contains(&settings.max_parallel) {
        return Err("fleet max_parallel must be 1-32".into());
    }
    if !(1..=200).contains(&settings.max_turns) {
        return Err("fleet max_turns must be 1-200".into());
    }
    if !(60..=7200).contains(&settings.timeout_secs) {
        return Err("fleet timeout_secs must be 60-7200".into());
    }
    serde_json::from_value(serde_json::json!(settings)).map_err(|e| e.to_string())
}
pub(crate) fn oracle_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: OracleSettings =
        serde_json::from_value(serde_json::json!(config)).map_err(|e| e.to_string())?;
    model_ok(settings.model.as_deref())?;
    timeout_ok(settings.timeout_secs)?;
    serde_json::from_value(serde_json::json!(settings)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn fleet_presets_roles_and_older_snapshots() {
        use crate::{
            agent::{AgentRole, CapabilitySnapshot},
            agent_capabilities::AgentCapabilities,
        };

        for preset in ["strict", "standard", "full"] {
            let config: AgentCapabilities =
                toml::from_str(&format!("preset = '{preset}'")).unwrap();
            for role in [AgentRole::Worker, AgentRole::Assistant] {
                let snapshot = config.resolve(role, None);
                assert_eq!(snapshot.enabled("fleet"), preset != "strict");
                if preset != "strict" {
                    let settings = &snapshot.features["fleet"].settings;
                    assert_eq!(settings["max_parallel"], 32);
                    assert_eq!(settings["max_turns"], 60);
                    assert_eq!(settings["timeout_secs"], 1800);
                }
            }
            for role in [
                AgentRole::Pm,
                AgentRole::Planner,
                AgentRole::PlanReviewer,
                AgentRole::Reviewer,
            ] {
                assert!(!config.resolve(role, None).features.contains_key("fleet"));
            }
        }
        for role in ["pm", "planner", "plan_reviewer", "reviewer"] {
            assert!(
                toml::from_str::<AgentCapabilities>(&format!("[{role}.fleet]\nenabled = true"))
                    .is_err()
            );
        }
        let old: AgentCapabilities =
            toml::from_str("preset = 'standard'\n[worker.subagents]\nmax_parallel = 2").unwrap();
        let mut snapshot = serde_json::to_value(old.resolve(AgentRole::Worker, None)).unwrap();
        snapshot["catalog"] = 10.into();
        snapshot["features"]
            .as_object_mut()
            .unwrap()
            .remove("fleet");
        let restored: CapabilitySnapshot = serde_json::from_value(snapshot).unwrap();
        assert!(!restored.enabled("fleet"));
        assert_eq!(restored.features["subagents"].settings["max_parallel"], 2);
    }

    #[test]
    fn fleet_settings_enforce_named_bounds_even_when_disabled() {
        use crate::agent_capabilities::AgentCapabilities;

        for (key, min, max) in [
            ("max_parallel", 1, 32),
            ("max_turns", 1, 200),
            ("timeout_secs", 60, 7200),
        ] {
            for value in [min - 1, max + 1] {
                let text = format!("[worker.fleet]\nenabled = false\n{key} = {value}");
                let error = toml::from_str::<AgentCapabilities>(&text)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(key), "{text}: {error}");
            }
            for value in [min, max] {
                let text = format!("[worker.fleet]\n{key} = {value}");
                let config: AgentCapabilities = toml::from_str(&text).unwrap();
                assert_eq!(
                    config
                        .resolve(crate::agent::AgentRole::Worker, None)
                        .features["fleet"]
                        .settings[key],
                    value
                );
            }
        }
    }

    use super::*;
    #[test]
    fn auxiliary_presets_roles_and_overrides_share_the_catalog() {
        use crate::{agent::AgentRole, agent_capabilities::AgentCapabilities};
        for preset in ["strict", "standard", "full"] {
            let config: AgentCapabilities =
                toml::from_str(&format!("preset = '{preset}'")).unwrap();
            for role in [
                AgentRole::Worker,
                AgentRole::Reviewer,
                AgentRole::Pm,
                AgentRole::Planner,
                AgentRole::PlanReviewer,
            ] {
                let snapshot = config.resolve(role, None);
                let explore =
                    preset == "full" || (preset == "standard" && role != AgentRole::Reviewer);
                assert_eq!(snapshot.enabled("subagents"), explore, "{preset}/{role:?}");
                assert!(
                    !snapshot.features.contains_key("oracle"),
                    "{preset}/{role:?}"
                );
                if explore {
                    assert_eq!(snapshot.features["subagents"].settings["max_parallel"], 32);
                }
            }
        }
        let enabled: AgentCapabilities = toml::from_str("preset = 'strict'\n[worker.subagents]\nenabled = true\nmax_parallel = 2\n[worker.oracle]\nenabled = true\nmodel = 'stronger'\n").unwrap();
        let snapshot = enabled.resolve(AgentRole::Worker, None);
        assert!(snapshot.enabled("subagents"));
        assert!(!snapshot.features.contains_key("oracle"));
        assert_eq!(snapshot.features["subagents"].settings["max_parallel"], 2);
        let disabled: AgentCapabilities = toml::from_str("preset = 'full'\n[worker.subagents]\nenabled = false\n[worker.oracle]\nenabled = false\n").unwrap();
        assert!(
            !disabled
                .resolve(AgentRole::Worker, None)
                .enabled("subagents")
        );
        assert!(
            !disabled
                .resolve(AgentRole::Worker, None)
                .features
                .contains_key("oracle")
        );
        assert!(toml::from_str::<AgentCapabilities>("[pm.subagents]\nenabled = true").is_ok());
    }
    #[test]
    fn legacy_oracle_config_and_snapshots_remain_readable() {
        use crate::{
            agent::{AgentRole, CapabilitySnapshot},
            agent_capabilities::AgentCapabilities,
        };
        for role in [
            "worker",
            "reviewer",
            "pm",
            "planner",
            "plan_reviewer",
            "assistant",
        ] {
            for value in [
                "oracle = true".to_string(),
                "[ROLE.oracle]\nenabled = true\nmodel = 'stronger'\ntimeout_secs = 120"
                    .replace("ROLE", role),
            ] {
                let text = if value.starts_with("oracle") {
                    format!("[{role}]\n{value}")
                } else {
                    value
                };
                let config: AgentCapabilities = toml::from_str(&text).unwrap();
                for agent_role in [
                    AgentRole::Worker,
                    AgentRole::Reviewer,
                    AgentRole::Pm,
                    AgentRole::Planner,
                    AgentRole::PlanReviewer,
                    AgentRole::Assistant,
                ] {
                    assert!(
                        !config
                            .resolve(agent_role, None)
                            .features
                            .contains_key("oracle")
                    );
                }
            }
        }
        assert!(toml::from_str::<AgentCapabilities>("[worker.oracle]\nmodel = ''").is_err());
        assert!(toml::from_str::<AgentCapabilities>("[worker.oracle]\nenabled = 'yes'").is_err());
        let config: AgentCapabilities = toml::from_str("preset = 'full'").unwrap();
        let mut old = serde_json::to_value(config.resolve(AgentRole::Worker, None)).unwrap();
        old["catalog"] = 11.into();
        old["features"]["oracle"] = serde_json::json!({"enabled": true, "source": "preset", "settings": {"model": "stronger", "timeout_secs": 120}});
        let restored: CapabilitySnapshot = serde_json::from_value(old).unwrap();
        assert!(restored.enabled("oracle"));
        assert_eq!(restored.features["oracle"].settings["model"], "stronger");
    }

    #[test]
    fn explore_service_tier_is_explicit_validated_and_absent_from_legacy_defaults() {
        let defaults = explore_settings(&CapabilitySettings::new()).unwrap();
        assert!(!defaults.contains_key("service_tier"));
        for tier in ["default", "priority"] {
            let settings =
                explore_settings(&[("service_tier".into(), tier.into())].into()).unwrap();
            assert_eq!(settings["service_tier"], tier);
        }
        for tier in ["fast", "flex", "bogus"] {
            assert!(explore_settings(&[("service_tier".into(), tier.into())].into()).is_err());
        }
        assert!(oracle_settings(&[("service_tier".into(), "priority".into())].into()).is_err());
        use crate::{agent::AgentRole, agent_capabilities::AgentCapabilities};
        let config: AgentCapabilities = toml::from_str(
            "[assistant.subagents]\nmodel = 'gpt-6-luna'\nservice_tier = 'priority'\n",
        )
        .unwrap();
        let snapshot = config.resolve(AgentRole::Assistant, None);
        assert_eq!(
            snapshot.features["subagents"].settings["service_tier"],
            "priority"
        );
    }

    #[test]
    fn bounded_auxiliary_settings() {
        let empty = CapabilitySettings::new();
        assert_eq!(
            oracle_settings(&empty).unwrap()["model"],
            serde_json::Value::Null
        );
        for key in ["max_parallel", "max_turns", "timeout_secs"] {
            assert!(explore_settings(&[(key.into(), 0.into())].into()).is_err());
        }
        let parallel = |n: u32| [("max_parallel".into(), n.into())].into();
        assert_eq!(explore_settings(&parallel(32)).unwrap()["max_parallel"], 32);
        assert!(explore_settings(&parallel(33)).is_err());
        assert!(oracle_settings(&[("model".into(), "".into())].into()).is_err());
    }
}
