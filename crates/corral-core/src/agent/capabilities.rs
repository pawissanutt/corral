//! The shared Pi-native capability catalog and its per-generation snapshot (T-563).
//!
//! One catalog owns every optional capability's key and per-role defaults; the resolver in
//! `config::agent_capabilities` turns configuration into a `CapabilitySnapshot`, which each
//! generation stores once and never rewrites. Host, doctor and state read that same snapshot.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::AgentRole;
use super::local_controls::{bash_rule_settings, hook_settings};
use super::paths::sha256_hex;
use crate::config::agent_capabilities::{
    browser_settings, compaction_settings, job_settings, skill_settings, template_settings,
};

/// A configured settings table, or the effective settings a snapshot records.
pub type CapabilitySettings = BTreeMap<String, Value>;

/// Validates a configured table (without `enabled`) and returns the effective settings.
pub type SettingsFn = fn(&CapabilitySettings) -> Result<CapabilitySettings, String>;

/// Bumped when a catalog entry's meaning or the snapshot's canonical form changes.
pub const CAPABILITY_CATALOG_VERSION: u32 = 5;

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
        key: "role_enabled",
        summary: "native planner or plan-reviewer host (off: launch is refused, never OmO)",
        defaults: plan_role_defaults,
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "session_export",
        summary: "/corral-export offline HTML session artifact",
        defaults: |role| (role != AgentRole::Pm).then_some([false, true, true]),
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "mcp.enabled",
        summary: "explicit project-scoped MCP servers",
        defaults: mcp_defaults,
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "codemode",
        summary: "QuickJS parallel tool calls and result filtering (no non-LLM models)",
        defaults: |role| Some([false, role != AgentRole::Pm, true]),
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "tool_search",
        summary: "local cached tool discovery",
        defaults: mcp_defaults,
        host: true,
        settings: None,
    },
    CapabilityDef {
        key: "lsp",
        summary: "read-only language server operations",
        defaults: lsp_defaults,
        host: true,
        settings: Some(lsp_settings),
    },
    CapabilityDef {
        key: "hooks",
        summary: "configured pre/post-tool and turn-end argv hooks",
        defaults: hooks_defaults,
        host: true,
        settings: Some(hook_settings),
    },
    CapabilityDef {
        key: "bash_rules",
        summary: "bash allow/ask/deny command rules",
        defaults: bash_rules_defaults,
        host: true,
        settings: Some(bash_rule_settings),
    },
    CapabilityDef {
        key: "background_shell",
        summary: "worker background jobs and receipted job monitors",
        defaults: background_shell_defaults,
        host: true,
        settings: Some(job_settings),
    },
    CapabilityDef {
        key: "browser",
        summary: "fresh-profile browser navigation/screenshot of dev origins and worktree HTML",
        defaults: browser_defaults,
        host: true,
        settings: Some(browser_settings),
    },
];

/// Validates configured servers and preserves their generation-bound settings.
fn lsp_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let mut extensions = std::collections::BTreeSet::new();
    for (key, value) in table {
        if key != "languages" {
            return Err(format!("has no setting {key} (known: languages)"));
        }
        let servers = value.as_object().ok_or("languages must be a table")?;
        for (id, value) in servers {
            let spec = value.as_object().ok_or("language server must be a table")?;
            if id.is_empty()
                || spec.keys().any(|k| {
                    !["command", "args", "extensions", "root_markers"].contains(&k.as_str())
                })
            {
                return Err(format!("invalid language server {id}"));
            }
            if !spec
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty() && !v.contains('\0'))
            {
                return Err(format!("{id}: command must be a nonempty string"));
            }
            for key in ["args", "root_markers"] {
                if let Some(value) = spec.get(key) {
                    let items = value
                        .as_array()
                        .ok_or_else(|| format!("{id}: {key} must be a list"))?;
                    if items.iter().any(|v| {
                        !v.as_str().is_some_and(|s| {
                            !s.contains('\0')
                                && (key != "root_markers"
                                    || (!s.is_empty() && s != "." && s != ".." && !s.contains('/')))
                        })
                    }) {
                        return Err(format!("{id}: invalid {key}"));
                    }
                }
            }
            let mappings = spec
                .get("extensions")
                .and_then(|v| v.as_object())
                .ok_or_else(|| format!("{id}: extensions must be a table"))?;
            if mappings.is_empty() {
                return Err(format!("{id}: extensions must not be empty"));
            }
            for (ext, language) in mappings {
                if !ext.starts_with('.')
                    || ext.len() < 2
                    || ext.contains('/')
                    || !extensions.insert(ext.clone())
                    || language.as_str().is_none_or(|s| s.trim().is_empty())
                {
                    return Err(format!("{id}: invalid or duplicate extension {ext}"));
                }
            }
        }
    }
    Ok(table.clone())
}

fn lsp_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker => Some([true, true, true]),
        AgentRole::Reviewer => Some([false, false, true]),
        AgentRole::Planner | AgentRole::PlanReviewer => Some([false, true, true]),
        AgentRole::Pm => None,
    }
}

fn browser_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker => Some([false, true, true]),
        AgentRole::Reviewer | AgentRole::Pm | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}

fn hooks_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker => Some([false, false, true]),
        AgentRole::Reviewer | AgentRole::Pm | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}

fn bash_rules_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker | AgentRole::Reviewer => Some([false, false, true]),
        AgentRole::Pm | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}

fn background_shell_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker => Some([false, true, true]),
        AgentRole::Reviewer | AgentRole::Pm | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}

fn optional_text(_: AgentRole) -> Option<[bool; 3]> {
    Some([false, true, true])
}

fn mcp_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker
        | AgentRole::Reviewer
        | AgentRole::Pm
        | AgentRole::Planner
        | AgentRole::PlanReviewer => Some([false, true, true]),
    }
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

/// Explicit workspace MCP policy; the host filters projects by its bound identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpSettings {
    pub servers: BTreeMap<String, McpServer>,
}

/// No ambient configuration or inline credential fields are accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, McpToolAccess>,
    #[serde(default)]
    pub resources: Vec<String>,
}

/// Configured tool effect ceiling, independent of server-provided annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpToolAccess {
    Read,
    Write,
}

// The exhaustive match is the role seam: a new role (T-544's assistant) must decide each entry.
fn todo_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker | AgentRole::Pm => Some([false, true, true]),
        AgentRole::Reviewer | AgentRole::Planner | AgentRole::PlanReviewer => None,
    }
}

/// Menu 19: only the native planning roles have an enable switch; other roles always run.
fn plan_role_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Planner | AgentRole::PlanReviewer => Some([false, true, true]),
        AgentRole::Worker | AgentRole::Reviewer | AgentRole::Pm => None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpSettings>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<&'a McpSettings>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_change: Option<McpSettingsChange>,
}

/// Concrete web policy difference without changing the boolean feature-change API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSettingsChange {
    pub from: Option<WebSettings>,
    pub to: Option<WebSettings>,
}

/// Concrete MCP policy difference, including project bindings and effect ceilings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSettingsChange {
    pub from: Option<McpSettings>,
    pub to: Option<McpSettings>,
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
        Self::with_settings(role, preset, features, web, None, project_text)
    }

    pub fn with_settings(
        role: AgentRole,
        preset: SnapshotPreset,
        features: BTreeMap<String, CapabilityState>,
        web: Option<WebSettings>,
        mcp: Option<McpSettings>,
        project_text: Option<bool>,
    ) -> Self {
        let web = web.filter(|_| {
            features
                .get("web.enabled")
                .is_some_and(|state| state.enabled)
        });
        let mcp = mcp.filter(|_| {
            features
                .get("mcp.enabled")
                .is_some_and(|state| state.enabled)
        });
        let canonical = Canonical {
            catalog: CAPABILITY_CATALOG_VERSION,
            role,
            preset,
            features: &features,
            web: web.as_ref(),
            project_text,
            mcp: mcp.as_ref(),
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
            mcp,
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
            mcp_change: (self.mcp != previous.mcp).then(|| McpSettingsChange {
                from: previous.mcp.clone(),
                to: self.mcp.clone(),
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
        if let Some(mcp) = &self.mcp {
            features.push(format!("mcp settings {mcp:?}"));
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
        if let Some(change) = &self.mcp_change {
            changes.push(format!("mcp settings {:?} -> {:?}", change.from, change.to));
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

#[cfg(test)]
mod lsp_config_tests {
    use super::*;
    use crate::config::agent_capabilities::{AgentCapabilities, Preset};

    #[test]
    fn lsp_configuration_is_validated_and_changes_generation_snapshot() {
        let text = "[worker.lsp.languages.example]\ncommand = 'server'\nargs = ['--stdio']\nextensions = { '.ex' = 'example' }\nroot_markers = ['example.toml']";
        let before: AgentCapabilities = toml::from_str(text).unwrap();
        let after: AgentCapabilities = toml::from_str(&text.replace("--stdio", "--other")).unwrap();
        let old = before.resolve(AgentRole::Worker, None);
        let new = after.resolve(AgentRole::Worker, None);
        assert_ne!(old.digest, new.digest);
        assert!(
            new.transition_from(1, &old)
                .unwrap()
                .changes
                .iter()
                .any(|c| c.key == "lsp" && c.settings)
        );
        assert!(
            toml::from_str::<AgentCapabilities>(&text.replace("example.toml", "../escape"))
                .is_err()
        );
        assert!(
            toml::from_str::<AgentCapabilities>(
                &text.replace("command = 'server'", "command = ''")
            )
            .is_err()
        );
        assert!(toml::from_str::<AgentCapabilities>(&text.replace("[worker.", "[pm.")).is_err());
        for (role, defaults) in [
            (AgentRole::Worker, [true, true, true]),
            (AgentRole::Reviewer, [false, false, true]),
            (AgentRole::Planner, [false, true, true]),
            (AgentRole::PlanReviewer, [false, true, true]),
        ] {
            for (preset, enabled) in [Preset::Strict, Preset::Standard, Preset::Full]
                .into_iter()
                .zip(defaults)
            {
                let mut config = AgentCapabilities::default();
                config.set_preset(preset);
                assert_eq!(config.resolve(role, None).enabled("lsp"), enabled);
            }
        }
    }
}

#[cfg(test)]
mod mcp_tests {
    use super::*;
    use crate::config::agent_capabilities::AgentCapabilities;

    #[test]
    fn mcp_config_enforces_host_name_and_tool_policy() {
        // Given server names at and beyond the host boundary.
        for (name, allowed) in [
            ("docs_1-local", true),
            ("bad.name", false),
            ("", false),
            ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", false),
        ] {
            // When parsing, then the Rust policy matches the host name constraints.
            let text =
                format!("[worker.mcp.servers.'{name}']\nprojects = ['corral']\ncommand = 'mcp'");
            assert_eq!(toml::from_str::<AgentCapabilities>(&text).is_ok(), allowed);
        }
        // Given tools including mislabeled execution proxies and Corral namespace tokens.
        for role in ["worker", "reviewer", "pm", "planner", "plan_reviewer"] {
            for tool in [
                "search",
                "run_command",
                "docs/EDIT",
                "corral_done",
                "docs.Corral.read",
            ] {
                let text = format!(
                    "[{role}.mcp.servers.docs]\nprojects = ['corral']\ncommand = 'mcp'\n\
                     tools = {{ '{tool}' = 'read' }}"
                );
                // When parsing, then Corral tokens are never allowed and proxies require worker.
                let allowed = tool == "search"
                    || (role == "worker" && ["run_command", "docs/EDIT"].contains(&tool));
                assert_eq!(
                    toml::from_str::<AgentCapabilities>(&text).is_ok(),
                    allowed,
                    "{role} {tool}"
                );
            }
        }
        // Omitted tool and resource allowlists default empty.
        let config: AgentCapabilities =
            toml::from_str("[worker.mcp.servers.docs]\nprojects = ['corral']\ncommand = 'mcp'")
                .unwrap();
        assert!(
            config.resolve(AgentRole::Worker, None).mcp.unwrap().servers["docs"]
                .tools
                .is_empty()
        );
    }

    #[test]
    fn mcp_policy_changes_digest_transition_and_round_trips() {
        // Given explicit settings with independent project, transport, tool and resource changes.
        let policy = "[worker.mcp.servers.docs]\nprojects = ['corral']\n\
                      command = 'docs-mcp'\ntools = { search = 'read' }\nresources = ['docs://a']";
        let resolve = |text: &str| {
            toml::from_str::<AgentCapabilities>(text)
                .unwrap()
                .resolve(AgentRole::Worker, None)
        };
        let previous = resolve(policy);
        let old_json = serde_json::to_string(&previous).unwrap();
        for changed in [
            policy.replace("corral", "other"),
            policy.replace("docs-mcp", "other-mcp"),
            policy.replace("'read'", "'write'"),
            policy.replace("docs://a", "docs://b"),
            format!("{policy}\nargs = ['--local']"),
            policy.replace("command = 'docs-mcp'", "url = 'https://example.org/mcp'"),
        ] {
            // When resolving a new generation.
            let current = resolve(&changed);
            let transition = current.transition_from(2, &previous).unwrap();
            // Then deterministic identity and concrete policy readback reflect that change.
            assert_eq!(current, resolve(&changed));
            assert_ne!(current.digest, previous.digest);
            assert!(transition.changes.is_empty());
            let change = transition.mcp_change.unwrap();
            assert_eq!(change.from, previous.mcp);
            assert_eq!(change.to, current.mcp);
            assert_eq!(
                serde_json::from_str::<CapabilitySnapshot>(
                    &serde_json::to_string(&current).unwrap()
                )
                .unwrap(),
                current
            );
        }
        assert_eq!(serde_json::to_string(&previous).unwrap(), old_json);
        assert_eq!(
            previous.mcp.as_ref().unwrap().servers["docs"].projects,
            ["corral"]
        );
    }

    #[test]
    fn disabled_or_unsupported_mcp_filters_settings_without_disabling_search() {
        // Given explicit server settings and either a disabled feature or an older host.
        let config: AgentCapabilities = toml::from_str(
            "[worker.mcp]\nenabled = false\n[worker.mcp.servers.docs]\n\
             projects = ['corral']\ncommand = 'docs-mcp'\ntools = {}",
        )
        .unwrap();
        let supported = ["tool_search".to_owned()].into_iter().collect();
        // When resolving a generation.
        for snapshot in [
            config.resolve(AgentRole::Worker, None),
            AgentCapabilities::default().resolve(AgentRole::Worker, Some(&supported)),
        ] {
            // Then MCP settings cannot reach launch, but local search remains independent.
            assert!(!snapshot.enabled("mcp.enabled"));
            assert!(snapshot.mcp.is_none());
            assert!(snapshot.enabled("tool_search"));
        }
    }

    #[test]
    fn pre_mcp_snapshots_and_transitions_remain_unchanged_when_parsed() {
        // Given recorded pre-MCP wire records with their original digests.
        let snapshot = r#"{"catalog":2,"role":"worker","preset":"standard","digest":"recorded"}"#;
        let transition = r#"{"from_generation":1,"from_digest":"old","from_preset":"strict"}"#;
        // When reading existing records.
        let old: CapabilitySnapshot = serde_json::from_str(snapshot).unwrap();
        let old_transition: CapabilityTransition = serde_json::from_str(transition).unwrap();
        // Then new fields remain absent and the recorded identity is never recomputed.
        assert!(old.mcp.is_none());
        assert_eq!(old.catalog, 2);
        assert_eq!(old.digest, "recorded");
        assert_eq!(
            serde_json::to_value(&old).unwrap(),
            serde_json::from_str::<serde_json::Value>(snapshot).unwrap()
        );
        assert!(old_transition.mcp_change.is_none());
    }
}
