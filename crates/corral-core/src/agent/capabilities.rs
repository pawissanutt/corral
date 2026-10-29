//! The shared Pi-native capability catalog and its per-generation snapshot (T-563).
//!
//! One catalog owns every optional capability's key and per-role defaults; the resolver in
//! `config::agent_capabilities` turns configuration into a `CapabilitySnapshot`, which each
//! generation stores once and never rewrites. Host, doctor and state read that same snapshot.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::AgentRole;
use super::paths::sha256_hex;

/// Bumped when a catalog entry's meaning or the snapshot's canonical form changes.
pub const CAPABILITY_CATALOG_VERSION: u32 = 1;

/// One catalog entry. `defaults` returns `[strict, standard, full]` for a role, or `None` where
/// the role's ceiling disallows the capability; a configured override can never cross it.
pub struct CapabilityDef {
    pub key: &'static str,
    pub summary: &'static str,
    pub defaults: fn(AgentRole) -> Option<[bool; 3]>,
}

/// Every capability Corral resolves. Each later member registers its own implemented key here.
pub const CATALOG: &[CapabilityDef] = &[CapabilityDef {
    key: "todo",
    summary: "corral_todo progress ledger",
    defaults: todo_defaults,
}];

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
}

/// The effective per-role settings of one generation. `digest` covers every other field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    pub catalog: u32,
    pub role: AgentRole,
    pub preset: SnapshotPreset,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub features: BTreeMap<String, CapabilityState>,
    pub digest: String,
}

#[derive(Serialize)]
struct Canonical<'a> {
    catalog: u32,
    role: AgentRole,
    preset: SnapshotPreset,
    features: &'a BTreeMap<String, CapabilityState>,
}

/// One setting that differs between two generations; `None` means absent from that snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityChange {
    pub key: String,
    pub from: Option<bool>,
    pub to: Option<bool>,
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
}

impl CapabilitySnapshot {
    pub fn new(
        role: AgentRole,
        preset: SnapshotPreset,
        features: BTreeMap<String, CapabilityState>,
    ) -> Self {
        let canonical = Canonical {
            catalog: CAPABILITY_CATALOG_VERSION,
            role,
            preset,
            features: &features,
        };
        // A derived Serialize over fixed fields and a BTreeMap cannot fail.
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        Self {
            catalog: CAPABILITY_CATALOG_VERSION,
            role,
            preset,
            features,
            digest: sha256_hex(&bytes),
        }
    }

    /// A generation recorded before the catalog: optional features are off except the
    /// wave-0 todo flag it recorded. Nothing about it is reconstructed or refused.
    pub fn legacy(role: AgentRole, todo: bool) -> Self {
        let features = CATALOG
            .iter()
            .filter(|def| (def.defaults)(role).is_some())
            .map(|def| {
                let state = CapabilityState {
                    enabled: def.key == "todo" && todo,
                    source: CapabilitySource::Legacy,
                    unavailable: None,
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
                let from = previous.features.get(key).map(|state| state.enabled);
                let to = self.features.get(key).map(|state| state.enabled);
                (from != to).then(|| CapabilityChange {
                    key: key.clone(),
                    from,
                    to,
                })
            })
            .collect();
        Some(CapabilityTransition {
            from_generation: generation,
            from_digest: previous.digest.clone(),
            from_preset: previous.preset,
            changes,
        })
    }

    pub fn summary(&self) -> String {
        let features: Vec<String> = self
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
        let changes: Vec<String> = self
            .changes
            .iter()
            .map(|change| {
                let side = |value: Option<bool>| match value {
                    Some(true) => "on",
                    Some(false) => "off",
                    None => "absent",
                };
                format!(
                    "{} {} -> {}",
                    change.key,
                    side(change.from),
                    side(change.to)
                )
            })
            .collect();
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
