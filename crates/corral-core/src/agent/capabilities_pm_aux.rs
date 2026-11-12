//! Opt-in PM auxiliary capabilities, omitted from disabled and legacy snapshots.
use super::super::auxiliary::{model_ok, timeout_ok};
use super::{AgentRole, CapabilityDef, CapabilitySettings, CapabilitySource, CapabilityState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const SHADOW: CapabilityDef = CapabilityDef {
    key: "shadow",
    summary: "host-only tool-free PM decision observation",
    defaults: pm_defaults,
    host: true,
    settings: Some(super::super::auxiliary::shadow_settings),
};
pub(super) const EXPERT: CapabilityDef = CapabilityDef {
    key: "expert",
    summary: "configured read-only PM expert consultation",
    defaults: pm_defaults,
    host: true,
    settings: Some(super::super::auxiliary::expert_settings),
};

fn pm_defaults(role: AgentRole) -> Option<[bool; 3]> {
    (role == AgentRole::Pm).then_some([false; 3])
}

impl CapabilityDef {
    pub(crate) fn omitted_when_disabled(&self) -> bool {
        matches!(self.key, "shadow" | "expert")
    }
}

pub(super) fn legacy_features(role: AgentRole, todo: bool) -> BTreeMap<String, CapabilityState> {
    super::CATALOG
        .iter()
        .filter(|def| !def.omitted_when_disabled())
        .filter_map(|def| Some((def, (def.defaults)(role)?)))
        .map(|(def, defaults)| {
            let state = CapabilityState {
                enabled: if def.key == "todo" { todo } else { defaults[0] },
                source: CapabilitySource::Legacy,
                unavailable: None,
                settings: CapabilitySettings::new(),
            };
            (def.key.to_owned(), state)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ShadowSettings {
    model: Option<String>,
    timeout_secs: u32,
    max_per_hour: u32,
}
impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            model: None,
            timeout_secs: 60,
            max_per_hour: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ExpertSettings {
    model: Option<String>,
    max_turns: u32,
    timeout_secs: u32,
    max_parallel: u32,
}
impl Default for ExpertSettings {
    fn default() -> Self {
        Self {
            model: None,
            max_turns: 24,
            timeout_secs: 600,
            max_parallel: 1,
        }
    }
}

pub(crate) fn shadow_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: ShadowSettings =
        serde_json::from_value(serde_json::json!(config)).map_err(|e| e.to_string())?;
    model_ok(settings.model.as_deref())?;
    timeout_ok(settings.timeout_secs)?;
    if !(1..=600).contains(&settings.max_per_hour) {
        return Err("shadow max_per_hour must be 1-600".into());
    }
    serde_json::from_value(serde_json::json!(settings)).map_err(|e| e.to_string())
}

pub(crate) fn expert_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: ExpertSettings =
        serde_json::from_value(serde_json::json!(config)).map_err(|e| e.to_string())?;
    model_ok(settings.model.as_deref())?;
    timeout_ok(settings.timeout_secs)?;
    if !(1..=64).contains(&settings.max_turns) || !(1..=16).contains(&settings.max_parallel) {
        return Err("expert needs max_turns=1-64 and max_parallel=1-16".into());
    }
    serde_json::from_value(serde_json::json!(settings)).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "capabilities_pm_aux_tests.rs"]
mod tests;
