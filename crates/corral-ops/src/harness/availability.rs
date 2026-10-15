use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::Ctx;
use crate::omo;

/// Preferred concrete OMO models for routing, sorted and deduplicated.
///
/// Reads `enabledModels` from `sync.omo_agent_dir`, then the existing OMO/SENPI
/// environment and HOME conventions. A readable auth object narrows the list to
/// providers with API-key or OAuth credentials. Missing or invalid auth leaves
/// the enabled list unfiltered. Credentials are not checked over the network.
///
/// `Some(vec![])` means explicitly no enabled models or no authenticated
/// intersection. `None` means missing, unreadable, malformed, or pattern-based
/// settings: callers should fall back to the T-003 harness discovery report.
/// Does not launch subprocesses, refresh discovery, or write its cache.
pub(crate) fn preferred_omo_models(ctx: &Ctx) -> Option<Vec<String>> {
    let dir = omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
    enabled_models(&dir)
}

#[derive(Deserialize)]
struct Settings {
    #[serde(rename = "enabledModels")]
    enabled_models: Vec<String>,
}

fn enabled_models(dir: &Path) -> Option<Vec<String>> {
    let settings = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    let Settings {
        enabled_models: mut models,
    } = serde_json::from_str(&settings).ok()?;
    // Patterns need the actual discovery list to expand them.
    if models.iter().any(|model| {
        model.contains(['*', '?'])
            || !model
                .split_once('/')
                .is_some_and(|(provider, id)| !provider.is_empty() && !id.is_empty())
    }) {
        return None;
    }
    if let Some(providers) = authenticated_providers(dir) {
        models.retain(|model| {
            model
                .split_once('/')
                .is_some_and(|(provider, _)| providers.contains_key(provider))
        });
    }
    models.sort();
    models.dedup();
    Some(models)
}

fn authenticated_providers(dir: &Path) -> Option<BTreeMap<String, Value>> {
    let auth = std::fs::read_to_string(dir.join("auth.json")).ok()?;
    let mut providers: BTreeMap<String, Value> = serde_json::from_str(&auth).ok()?;
    providers.retain(|_, credential| {
        let nonempty = |field| {
            credential
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        };
        match credential.get("type").and_then(Value::as_str) {
            Some("api_key") => nonempty("key"),
            Some("oauth") => nonempty("access") || nonempty("refresh"),
            _ => false,
        }
    });
    Some(providers)
}
