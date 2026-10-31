//! Generation-bound explore/oracle settings. Provider credentials remain the parent's.
use super::CapabilitySettings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExploreSettings {
    pub max_parallel: u32,
    pub model: Option<String>,
    pub max_turns: u32,
    pub timeout_secs: u32,
}
impl Default for ExploreSettings {
    fn default() -> Self {
        Self {
            max_parallel: 3,
            model: None,
            max_turns: 12,
            timeout_secs: 120,
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
fn model_ok(model: Option<&str>) -> Result<(), String> {
    if model
        .is_some_and(|s| s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control))
    {
        return Err("auxiliary model must be 1-256 non-control characters".into());
    }
    Ok(())
}
fn timeout_ok(timeout: u32) -> Result<(), String> {
    if !(1..=600).contains(&timeout) {
        return Err("auxiliary timeout_secs must be 1-600".into());
    }
    Ok(())
}
pub(crate) fn explore_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: ExploreSettings =
        serde_json::from_value(serde_json::json!(config)).map_err(|e| e.to_string())?;
    if !(1..=16).contains(&settings.max_parallel) || !(1..=64).contains(&settings.max_turns) {
        return Err("subagents needs max_parallel=1-16 and max_turns=1-64".into());
    }
    model_ok(settings.model.as_deref())?;
    timeout_ok(settings.timeout_secs)?;
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
                let explore = role != AgentRole::Pm
                    && (preset == "full" || (preset == "standard" && role != AgentRole::Reviewer));
                assert_eq!(snapshot.enabled("subagents"), explore, "{preset}/{role:?}");
                assert_eq!(
                    snapshot.enabled("oracle"),
                    preset == "full",
                    "{preset}/{role:?}"
                );
                if explore {
                    assert_eq!(snapshot.features["subagents"].settings["max_parallel"], 3);
                }
            }
        }
        let enabled: AgentCapabilities = toml::from_str("preset = 'strict'\n[worker.subagents]\nenabled = true\nmax_parallel = 2\n[worker.oracle]\nenabled = true\nmodel = 'stronger'\n").unwrap();
        let snapshot = enabled.resolve(AgentRole::Worker, None);
        assert!(snapshot.enabled("subagents") && snapshot.enabled("oracle"));
        assert_eq!(snapshot.features["subagents"].settings["max_parallel"], 2);
        assert_eq!(snapshot.features["oracle"].settings["model"], "stronger");
        let disabled: AgentCapabilities = toml::from_str("preset = 'full'\n[worker.subagents]\nenabled = false\n[worker.oracle]\nenabled = false\n").unwrap();
        assert!(
            !disabled
                .resolve(AgentRole::Worker, None)
                .enabled("subagents")
        );
        assert!(!disabled.resolve(AgentRole::Worker, None).enabled("oracle"));
        assert!(toml::from_str::<AgentCapabilities>("[pm.subagents]\nenabled = true").is_err());
    }
    #[test]
    fn bounded_auxiliary_settings() {
        let empty = CapabilitySettings::new();
        assert_eq!(explore_settings(&empty).unwrap()["max_parallel"], 3);
        assert_eq!(
            oracle_settings(&empty).unwrap()["model"],
            serde_json::Value::Null
        );
        for key in ["max_parallel", "max_turns", "timeout_secs"] {
            assert!(explore_settings(&[(key.into(), 0.into())].into()).is_err());
        }
        assert!(oracle_settings(&[("model".into(), "".into())].into()).is_err());
    }
}
