use std::path::Path;

use corral_core::agent::CapabilitySnapshot;
use serde_json::json;

/// The host-facing capability grant, shared by bound and untethered launches.
pub(crate) fn capabilities_payload(
    snapshot: &CapabilitySnapshot,
    project_dir: Option<&Path>,
) -> serde_json::Value {
    let mut payload = json!({
        "catalog": snapshot.catalog, "digest": snapshot.digest,
        // Rust-side keys never reach the host, which refuses keys it lacks.
        "preset": snapshot.preset.as_str(), "enabled": snapshot.enabled_keys()
            .filter(|key| corral_core::agent::capability(key).is_some_and(|def| def.host))
            .collect::<Vec<_>>(),
        "settings": snapshot.features.iter()
            .filter(|(key, state)| key.as_str() != "oracle" && state.enabled && !state.settings.is_empty())
            .map(|(key, state)| (key.clone(), json!(state.settings)))
            .collect::<serde_json::Map<_, _>>(),
        "project_text": snapshot.project_text == Some(true),
        "project_dir": project_dir,
    });
    if snapshot.enabled("web.enabled")
        && let Some(web) = &snapshot.web
    {
        payload["web"] = json!(web);
    }
    if snapshot.enabled("mcp.enabled")
        && let Some(mcp) = &snapshot.mcp
    {
        payload["mcp"] = json!(mcp);
    }
    if snapshot.enabled("llama_cpp.enabled")
        && let Some(local) = &snapshot.llama_cpp
    {
        payload["llama_cpp"] = json!(local);
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::{
        agent::{AgentRole, CapabilitySnapshot},
        agent_capabilities::AgentCapabilities,
    };

    #[test]
    fn full_and_historical_payloads_do_not_advertise_oracle() {
        let config: AgentCapabilities = serde_json::from_value(json!({
            "preset": "full", "worker": {"oracle": {"enabled": true, "model": "stronger"}}
        }))
        .unwrap();
        let snapshot = config.resolve(AgentRole::Worker, None);
        let payload = capabilities_payload(&snapshot, None);
        assert!(
            payload["enabled"]
                .as_array()
                .unwrap()
                .iter()
                .all(|key| key != "oracle")
        );
        assert!(payload["settings"].get("oracle").is_none());
        assert!(
            payload["enabled"]
                .as_array()
                .unwrap()
                .iter()
                .any(|key| key == "subagents")
        );

        let mut historical = serde_json::to_value(snapshot).unwrap();
        historical["catalog"] = 11.into();
        historical["features"]["oracle"] = json!({"enabled": true, "source": "preset", "settings": {"model": "stronger", "timeout_secs": 120}});
        let historical: CapabilitySnapshot = serde_json::from_value(historical).unwrap();
        let payload = capabilities_payload(&historical, None);
        assert!(
            payload["enabled"]
                .as_array()
                .unwrap()
                .iter()
                .all(|key| key != "oracle")
        );
        assert!(payload["settings"].get("oracle").is_none());
    }
}
