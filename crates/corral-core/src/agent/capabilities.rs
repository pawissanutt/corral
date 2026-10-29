//! The shared Pi-native capability catalog and its per-generation snapshot (T-563).
//!
//! One catalog owns every optional capability's key and per-role defaults; the resolver in
//! `config::agent_capabilities` turns configuration into a `CapabilitySnapshot`, which each
//! generation stores once and never rewrites. Host, doctor and state read that same snapshot.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::AgentRole;
use super::paths::sha256_hex;
use crate::config::agent_capabilities::{compaction_settings, skill_settings, template_settings};

/// A configured settings table, or the effective settings a snapshot records.
pub type CapabilitySettings = BTreeMap<String, Value>;

/// Validates a configured table (without `enabled`) and returns the effective settings.
pub type SettingsFn = fn(&CapabilitySettings) -> Result<CapabilitySettings, String>;

/// Bumped when a catalog entry's meaning or the snapshot's canonical form changes.
pub const CAPABILITY_CATALOG_VERSION: u32 = 2;

/// One catalog entry. `defaults` returns `[strict, standard, full]` for a role, or `None` where
/// the role's ceiling disallows the capability; a configured override can never cross it.
pub struct CapabilityDef {
    pub key: &'static str,
    pub summary: &'static str,
    pub defaults: fn(AgentRole) -> Option<[bool; 3]>,
    /// The installed Pi host must implement it; a Rust-side feature (lessons) never needs it.
    pub host: bool,
    /// Settings beside `enabled`; `None` means the key is a plain boolean.
    pub settings: Option<SettingsFn>,
}

/// Every capability Corral resolves. Each later member registers its own implemented key here.
pub const CATALOG: &[CapabilityDef] = &[
    CapabilityDef {
        key: "todo",
        summary: "corral_todo progress ledger",
        defaults: todo_defaults,
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "web.enabled",
        summary: "GET-only web fetch and search",
        defaults: web_defaults,
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "context_files",
        summary: "AGENTS.md/CLAUDE.md from a trusted project",
        defaults: optional_text,
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "skills",
        summary: "Agent Skills from user_dirs and a trusted project",
        defaults: optional_text,
        host: true,
        settings: Some(skill_settings),
    },
    CapabilityDef {
        key: "prompt_templates",
        summary: "prompt templates from user_dirs and a trusted project",
        defaults: optional_text,
        host: true,
        settings: Some(template_settings),
    },
    CapabilityDef {
        key: "compaction",
        summary: "automatic compaction and branch-summary settings",
        defaults: |_| Some([true, true, true]),
        host: true,
        settings: Some(compaction_settings),
    },
    CapabilityDef {
        key: "lessons",
        summary: "project lessons in the dispatch prompt",
        defaults: |role| (role != AgentRole::Pm).then_some([true, true, true]),
        host: false,
        settings: None,
    },
    CapabilityDef {
        key: "session_export",
        summary: "/corral-export offline HTML session artifact",
        defaults: |role| (role != AgentRole::Pm).then_some([false, true, true]),
        host: true,
        settings: None,
    },
];

fn optional_text(_: AgentRole) -> Option<[bool; 3]> {
    Some([false, true, true])
}

fn web_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker | AgentRole::Reviewer | AgentRole::Planner | AgentRole::PlanReviewer => {
            Some([false, true, true])
        }
        AgentRole::Pm => None,
    }
}

/// Resolved GET-only web policy, shared by snapshots and the host launch payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSettings {
    pub hosts: Vec<String>,
    pub max_bytes: u64,
    pub timeout_ms: u32,
    pub max_redirects: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searxng_url: Option<String>,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            hosts: vec!["*".into()],
            max_bytes: 1_048_576,
            timeout_ms: 15_000,
            max_redirects: 5,
            searxng_url: None,
        }
    }
}

// The exhaustive match is the role seam: a new role (T-544's assistant) must decide each entry.
fn todo_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker | AgentRole::Pm => Some([false, true, true]),
        AgentRole::Reviewer | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}

pub fn capability(key: &str) -> Option<&'static CapabilityDef> {
    CATALOG.iter().find(|def| def.key == key)
}

/// The preset a snapshot was resolved from; `legacy_strict` labels a generation that predates
/// the catalog and therefore has only its recorded wave-0 todo flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPreset {
    Strict,
    Standard,
    Full,
    LegacyStrict,
}

impl SnapshotPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            SnapshotPreset::Strict => "strict",
            SnapshotPreset::Standard => "standard",
            SnapshotPreset::Full => "full",
            SnapshotPreset::LegacyStrict => "legacy_strict",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySource {
    Preset,
    RoleOverride,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityState {
    /// Effective: requested and implemented by the installed host.
    pub enabled: bool,
    pub source: CapabilitySource,
    /// Requested but omitted because the installed host does not implement it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Effective settings of an enabled key that takes them; part of the digest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: CapabilitySettings,
}

/// The effective per-role settings of one generation. `digest` covers every other field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    pub catalog: u32,
    pub role: AgentRole,
    pub preset: SnapshotPreset,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub features: BTreeMap<String, CapabilityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<WebSettings>,
    /// The project-text trust decision (`trusted_projects`) for the launched project; absent
    /// when no project was resolved (doctor) or in a snapshot that predates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_text: Option<bool>,
    pub digest: String,
}

#[derive(Serialize)]
struct Canonical<'a> {
    catalog: u32,
    role: AgentRole,
    preset: SnapshotPreset,
    features: &'a BTreeMap<String, CapabilityState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web: Option<&'a WebSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_text: Option<bool>,
}

/// One setting that differs between two generations; `None` means absent from that snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityChange {
    pub key: String,
    pub from: Option<bool>,
    pub to: Option<bool>,
    /// The key's effective settings differ, whatever its enabled state.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub settings: bool,
}

/// Recorded on a generation whose snapshot differs from its predecessor's. The predecessor's
/// own snapshot stays unchanged for classifying its interrupted calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityTransition {
    pub from_generation: u32,
    pub from_digest: String,
    pub from_preset: SnapshotPreset,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<CapabilityChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_change: Option<WebSettingsChange>,
}

/// Concrete web policy difference without changing the boolean feature-change API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSettingsChange {
    pub from: Option<WebSettings>,
    pub to: Option<WebSettings>,
}

impl CapabilitySnapshot {
    pub fn new(
        role: AgentRole,
        preset: SnapshotPreset,
        features: BTreeMap<String, CapabilityState>,
    ) -> Self {
        Self::resolved(role, preset, features, None, None)
    }

    pub fn with_web(
        role: AgentRole,
        preset: SnapshotPreset,
        features: BTreeMap<String, CapabilityState>,
        web: Option<WebSettings>,
    ) -> Self {
        Self::resolved(role, preset, features, web, None)
    }

    /// A snapshot with web settings (kept only while web is enabled) and project-text trust.
    pub fn resolved(
        role: AgentRole,
        preset: SnapshotPreset,
        features: BTreeMap<String, CapabilityState>,
        web: Option<WebSettings>,
        project_text: Option<bool>,
    ) -> Self {
        let web = web.filter(|_| {
            features
                .get("web.enabled")
                .is_some_and(|state| state.enabled)
        });
        let canonical = Canonical {
            catalog: CAPABILITY_CATALOG_VERSION,
            role,
            preset,
            features: &features,
            web: web.as_ref(),
            project_text,
        };
        // A derived Serialize over fixed fields and a BTreeMap cannot fail.
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        Self {
            catalog: CAPABILITY_CATALOG_VERSION,
            role,
            preset,
            features,
            web,
            project_text,
            digest: sha256_hex(&bytes),
        }
    }

    /// A generation recorded before the catalog: optional features take their strict values
    /// (what such a host ran: upstream compaction and prompt lessons) except the wave-0 todo
    /// flag it recorded. Nothing about it is reconstructed or refused.
    pub fn legacy(role: AgentRole, todo: bool) -> Self {
        let features = CATALOG
            .iter()
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
            .collect();
        Self::new(role, SnapshotPreset::LegacyStrict, features)
    }

    pub fn enabled(&self, key: &str) -> bool {
        self.features.get(key).is_some_and(|state| state.enabled)
    }

    pub fn enabled_keys(&self) -> impl Iterator<Item = &str> {
        self.features
            .iter()
            .filter(|(_, state)| state.enabled)
            .map(|(key, _)| key.as_str())
    }

    /// The first explicitly requested capability the installed host lacks, with its reason.
    pub fn explicit_unavailable(&self) -> Option<(&str, &str)> {
        self.features.iter().find_map(|(key, state)| {
            (state.source == CapabilitySource::RoleOverride)
                .then_some(state.unavailable.as_deref())
                .flatten()
                .map(|reason| (key.as_str(), reason))
        })
    }

    /// `None` when both snapshots are identical.
    pub fn transition_from(
        &self,
        generation: u32,
        previous: &CapabilitySnapshot,
    ) -> Option<CapabilityTransition> {
        if self.digest == previous.digest {
            return None;
        }
        let mut keys: Vec<&String> = previous
            .features
            .keys()
            .chain(self.features.keys())
            .collect();
        keys.sort();
        keys.dedup();
        let changes = keys
            .into_iter()
            .filter_map(|key| {
                let (old, new) = (previous.features.get(key), self.features.get(key));
                let from = old.map(|state| state.enabled);
                let to = new.map(|state| state.enabled);
                let settings = old.map(|state| &state.settings) != new.map(|state| &state.settings);
                (from != to || settings).then(|| CapabilityChange {
                    key: key.clone(),
                    from,
                    to,
                    settings,
                })
            })
            .collect();
        Some(CapabilityTransition {
            from_generation: generation,
            from_digest: previous.digest.clone(),
            from_preset: previous.preset,
            changes,
            web_change: (self.web != previous.web).then(|| WebSettingsChange {
                from: previous.web.clone(),
                to: self.web.clone(),
            }),
        })
    }

    pub fn summary(&self) -> String {
        let mut features: Vec<String> = self
            .features
            .iter()
            .map(|(key, state)| {
                let source = match state.source {
                    CapabilitySource::Preset => "preset",
                    CapabilitySource::RoleOverride => "role override",
                    CapabilitySource::Legacy => "legacy record",
                };
                match &state.unavailable {
                    Some(reason) => format!("{key} unavailable ({source}: {reason})"),
                    None if state.enabled => format!("{key} on ({source})"),
                    None => format!("{key} off ({source})"),
                }
            })
            .collect();
        if let Some(web) = &self.web {
            features.push(format!(
                "web hosts={:?}, max_bytes={}, timeout_ms={}, max_redirects={}, searxng={}",
                web.hosts,
                web.max_bytes,
                web.timeout_ms,
                web.max_redirects,
                web.searxng_url.as_deref().unwrap_or("not configured")
            ));
        }
        format!(
            "{} {} [{}]: {}",
            self.role.as_str(),
            self.preset.as_str(),
            self.digest.get(..12).unwrap_or(&self.digest),
            if features.is_empty() {
                "no optional capabilities".to_owned()
            } else {
                features.join(", ")
            }
        )
    }
}

impl CapabilityTransition {
    pub fn summary(&self) -> String {
        let mut changes: Vec<String> = self
            .changes
            .iter()
            .map(|change| {
                let side = |value: Option<bool>| match value {
                    Some(true) => "on",
                    Some(false) => "off",
                    None => "absent",
                };
                format!(
                    "{} {} -> {}{}",
                    change.key,
                    side(change.from),
                    side(change.to),
                    if change.settings {
                        " (settings changed)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        if let Some(change) = &self.web_change {
            changes.push(format!("web settings {:?} -> {:?}", change.from, change.to));
        }
        format!(
            "since generation {} ({} [{}]): {}",
            self.from_generation,
            self.from_preset.as_str(),
            self.from_digest.get(..12).unwrap_or(&self.from_digest),
            if changes.is_empty() {
                "settings re-resolved with no feature change".to_owned()
            } else {
                changes.join(", ")
            }
        )
    }
}
