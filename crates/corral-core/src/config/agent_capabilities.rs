//! `[agent.capabilities]`: preset plus per-role overrides, resolved against the shared catalog
//! in `agent::capabilities` into one snapshot per generation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::agent::{
    AgentRole, CATALOG, CapabilitySettings, CapabilitySnapshot, CapabilitySource, CapabilityState,
    SnapshotPreset, WebSettings,
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

/// One `[agent.capabilities.<role>]` entry: `key = bool`, or a compound key's table with an
/// optional `enabled` and its settings (`[agent.capabilities.worker.skills] user_dirs = []`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RoleSetting {
    Enabled(bool),
    Table(CapabilitySettings),
}

/// `[agent.capabilities.<role>.web]`: optional preset override and GET-only web policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub settings: WebSettings,
}

/// `[agent.capabilities.<role>]`: boolean overrides, compound tables and web settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<WebCapabilities>,
    #[serde(flatten)]
    overrides: BTreeMap<String, RoleSetting>,
}

impl RoleCapabilities {
    pub fn get(&self, key: &str) -> Option<bool> {
        if key == "web.enabled"
            && let Some(enabled) = self.web.as_ref().and_then(|web| web.enabled)
        {
            return Some(enabled);
        }
        match self.overrides.get(key)? {
            RoleSetting::Enabled(enabled) => Some(*enabled),
            RoleSetting::Table(table) => table.get("enabled").and_then(|value| value.as_bool()),
        }
    }

    /// The configured settings of a compound key, without `enabled`.
    pub fn settings(&self, key: &str) -> CapabilitySettings {
        match self.overrides.get(key) {
            Some(RoleSetting::Table(table)) => table
                .iter()
                .filter(|(name, _)| name.as_str() != "enabled")
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            _ => CapabilitySettings::new(),
        }
    }

    pub fn set(&mut self, key: &str, enabled: bool) {
        if key == "web.enabled" {
            self.overrides.remove(key);
            self.web.get_or_insert_default().enabled = Some(enabled);
            return;
        }
        match self.overrides.get_mut(key) {
            Some(RoleSetting::Table(table)) => {
                table.insert("enabled".into(), enabled.into());
            }
            _ => {
                self.overrides
                    .insert(key.to_owned(), RoleSetting::Enabled(enabled));
            }
        }
    }

    /// Replaces a compound key's settings, keeping its `enabled` override.
    pub fn set_settings(&mut self, key: &str, settings: CapabilitySettings) {
        let mut table = settings;
        if let Some(enabled) = self.get(key) {
            table.insert("enabled".into(), enabled.into());
        }
        self.overrides
            .insert(key.to_owned(), RoleSetting::Table(table));
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty() && self.web.is_none()
    }
}

/// `trusted_projects` default: every registered project's text is trusted.
fn all_projects() -> Vec<String> {
    vec!["*".into()]
}

fn trusts_all(projects: &[String]) -> bool {
    projects == all_projects().as_slice()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RawCapabilities {
    pub preset: Preset,
    /// Projects whose repository text (AGENTS.md, CLAUDE.md, `.agents/skills`,
    /// `.agents/prompts`) a host may load; `"*"` trusts every registered project, `[]` none.
    #[serde(default = "all_projects", skip_serializing_if = "trusts_all")]
    pub trusted_projects: Vec<String>,
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

impl Default for RawCapabilities {
    fn default() -> Self {
        Self {
            preset: Preset::default(),
            trusted_projects: all_projects(),
            worker: RoleCapabilities::default(),
            reviewer: RoleCapabilities::default(),
            pm: RoleCapabilities::default(),
            planner: RoleCapabilities::default(),
            plan_reviewer: RoleCapabilities::default(),
        }
    }
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
            let settings = config.role(role);
            if let Some(web) = &settings.web {
                if web.enabled.is_some() && settings.overrides.contains_key("web.enabled") {
                    return Err("web.enabled is configured both as a key and a web setting".into());
                }
                validate_web(&web.settings)?;
            }
            let entries = settings
                .overrides
                .iter()
                .map(|(key, setting)| (key.as_str(), Some(setting)))
                .chain(settings.web.as_ref().map(|_| ("web.enabled", None)));
            for (key, setting) in entries {
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
                let Some(RoleSetting::Table(table)) = setting else {
                    continue;
                };
                let table_name = format!("[agent.capabilities.{}.{key}]", role.as_str());
                if table
                    .get("enabled")
                    .is_some_and(|value| !value.is_boolean())
                {
                    return Err(format!("{table_name} enabled must be a boolean"));
                }
                let settings = config.role(role).settings(key);
                match def.settings {
                    Some(check) => {
                        check(&settings).map_err(|reason| format!("{table_name} {reason}"))?;
                    }
                    None if settings.is_empty() => {}
                    None => return Err(format!("{table_name} takes only `enabled`")),
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

    pub fn set_trusted_projects(&mut self, projects: Vec<String>) {
        self.0.trusted_projects = projects;
    }

    /// Whether `project`'s repository text may be loaded (`trusted_projects`).
    pub fn trusts(&self, project: &str) -> bool {
        self.0
            .trusted_projects
            .iter()
            .any(|name| name == "*" || name == project)
    }

    fn edit(
        &mut self,
        role: AgentRole,
        change: impl FnOnce(&mut RoleCapabilities),
    ) -> Result<(), String> {
        let mut raw = self.0.clone();
        change(match role {
            AgentRole::Worker => &mut raw.worker,
            AgentRole::Reviewer => &mut raw.reviewer,
            AgentRole::Pm => &mut raw.pm,
            AgentRole::Planner => &mut raw.planner,
            AgentRole::PlanReviewer => &mut raw.plan_reviewer,
        });
        *self = Self::try_from(raw)?;
        Ok(())
    }

    /// Sets a role override after checking it against the catalog, as parsing does.
    pub fn set(&mut self, role: AgentRole, key: &str, enabled: bool) -> Result<(), String> {
        self.edit(role, |table| table.set(key, enabled))
    }

    /// Sets a compound key's settings for `role`, checked as parsing checks them.
    pub fn set_settings(
        &mut self,
        role: AgentRole,
        key: &str,
        settings: CapabilitySettings,
    ) -> Result<(), String> {
        self.edit(role, |table| table.set_settings(key, settings))
    }

    /// The effective snapshot for `role` without a project, as doctor shows every role.
    pub fn resolve(
        &self,
        role: AgentRole,
        supported: Option<&BTreeSet<String>>,
    ) -> CapabilitySnapshot {
        self.resolve_for(role, None, supported)
    }

    /// The effective snapshot for `role` launched in `project`. `supported` is the installed
    /// host's implemented keys; `None` skips that check. A requested key the host lacks is
    /// reported and left off. The project's text trust is part of the snapshot.
    pub fn resolve_for(
        &self,
        role: AgentRole,
        project: Option<&str>,
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
                let lacking = def.host && supported.is_some_and(|keys| !keys.contains(def.key));
                let unavailable = (wanted && lacking).then(|| {
                    format!(
                        "the installed Pi host does not implement {}; run `corral agent install` \
                             to update it",
                        def.key
                    )
                });
                let enabled = wanted && unavailable.is_none();
                // Parsing validated the table, so a settings error cannot occur here.
                let settings = match def.settings {
                    Some(effective) if enabled => {
                        effective(&overrides.settings(def.key)).unwrap_or_default()
                    }
                    _ => CapabilitySettings::new(),
                };
                let state = CapabilityState {
                    enabled,
                    source,
                    unavailable,
                    settings,
                };
                Some((def.key.to_owned(), state))
            })
            .collect();
        let web = overrides
            .web
            .as_ref()
            .map_or_else(WebSettings::default, |web| web.settings.clone());
        let trust = project.map(|project| self.trusts(project));
        CapabilitySnapshot::resolved(role, self.0.preset.snapshot(), features, Some(web), trust)
    }

    pub fn todo(&self, role: AgentRole) -> bool {
        self.resolve(role, None).enabled("todo")
    }
}

fn validate_web(web: &WebSettings) -> Result<(), String> {
    if web.max_bytes == 0 || web.max_bytes > 9_007_199_254_740_991 {
        return Err("web.max_bytes must be a positive JavaScript safe integer".into());
    }
    if web.timeout_ms == 0 || web.timeout_ms > 2_147_483_647 {
        return Err("web.timeout_ms must be between 1 and 2147483647".into());
    }
    if web.max_redirects > 10 {
        return Err("web.max_redirects must be between 0 and 10".into());
    }
    if let Some(url) = &web.searxng_url {
        let endpoint = regex::Regex::new(
            r"^https?://([A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?|\[[0-9A-Fa-f:]+\])(?::([0-9]+))?(?:[/?][^\s#\\]*)?$",
        ).map_err(|error| format!("compiling web endpoint pattern: {error}"))?;
        let captures = endpoint
            .captures(url)
            .ok_or("web.searxng_url must be an HTTP(S) endpoint without userinfo or fragments")?;
        if let Some(port) = captures.get(2)
            && port.as_str().parse::<u16>().is_err()
        {
            return Err("web.searxng_url has an invalid port".into());
        }
        if let Some(host) = captures.get(1)
            && host.as_str().starts_with('[')
            && host.as_str()[1..host.as_str().len() - 1]
                .parse::<std::net::Ipv6Addr>()
                .is_err()
        {
            return Err("web.searxng_url has an invalid IPv6 host".into());
        }
    }
    Ok(())
}

/// A JSON safe integer: the host passes token counts to upstream as JavaScript numbers.
const SAFE_INTEGER_MAX: u64 = (1 << 53) - 1;

fn tokens(value: &serde_json::Value, name: &str) -> Result<serde_json::Value, String> {
    match value.as_u64() {
        Some(count) if count <= SAFE_INTEGER_MAX => Ok(count.into()),
        _ => Err(format!("{name} must be a non-negative integer token count")),
    }
}

fn unknown(name: &str, known: &str) -> String {
    format!("has no setting {name} (known: {known})")
}

/// `user_dirs`: absolute or `~/` directories, read as text only; `[]` loads none.
fn user_dirs(table: &CapabilitySettings, default: &str) -> Result<CapabilitySettings, String> {
    let mut dirs = vec![serde_json::Value::from(default)];
    for (name, value) in table {
        if name != "user_dirs" {
            return Err(unknown(name, "user_dirs"));
        }
        let list = value
            .as_array()
            .ok_or("user_dirs must be a list of directories")?;
        for dir in list {
            let text = dir.as_str().unwrap_or_default();
            if !(text.starts_with('/') || text.starts_with("~/")) || text.contains('\0') {
                return Err(format!(
                    "user_dirs entry {dir} must be an absolute or ~/ directory"
                ));
            }
        }
        dirs.clone_from(list);
    }
    Ok([("user_dirs".to_owned(), dirs.into())].into())
}

pub(crate) fn skill_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    user_dirs(table, "~/.agents/skills")
}

pub(crate) fn template_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    user_dirs(table, "~/.agents/prompts")
}

/// Compaction keeps upstream's defaults for every value left out, so only configured values
/// are recorded: `reserve_tokens`, `keep_recent_tokens` and `branch_summary.{reserve_tokens,
/// skip_prompt}`.
pub(crate) fn compaction_settings(
    table: &CapabilitySettings,
) -> Result<CapabilitySettings, String> {
    let mut effective = CapabilitySettings::new();
    for (name, value) in table {
        let value = match name.as_str() {
            "reserve_tokens" | "keep_recent_tokens" => tokens(value, name)?,
            "branch_summary" => {
                let summary = value.as_object().ok_or("branch_summary must be a table")?;
                let mut out = serde_json::Map::new();
                for (inner, item) in summary {
                    let checked = match inner.as_str() {
                        "reserve_tokens" => tokens(item, "branch_summary.reserve_tokens")?,
                        "skip_prompt" => item
                            .as_bool()
                            .ok_or("branch_summary.skip_prompt must be a boolean")?
                            .into(),
                        other => {
                            return Err(unknown(
                                &format!("branch_summary.{other}"),
                                "reserve_tokens, skip_prompt",
                            ));
                        }
                    };
                    out.insert(inner.clone(), checked);
                }
                out.into()
            }
            other => {
                return Err(unknown(
                    other,
                    "reserve_tokens, keep_recent_tokens, branch_summary",
                ));
            }
        };
        effective.insert(name.clone(), value);
    }
    Ok(effective)
}

#[cfg(test)]
#[path = "agent_capabilities_tests.rs"]
mod tests;
