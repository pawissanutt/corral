//! `[agent.capabilities]`: preset plus per-role overrides, resolved against the shared catalog
//! in `agent::capabilities` into one snapshot per generation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::agent::{
    AgentRole, CATALOG, CapabilitySettings, CapabilitySnapshot, CapabilitySource, CapabilityState,
    LlamaCppSettings, McpSettings, McpToolAccess, SnapshotPreset, WebSettings,
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

/// `[agent.capabilities.<role>.mcp]`: explicit server policy and preset override.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub settings: McpSettings,
}

/// `[agent.capabilities.<role>.llama_cpp]`: explicit local provider opt-in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlamaCppCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub settings: LlamaCppSettings,
}

/// `[agent.capabilities.<role>]`: boolean overrides, compound tables and typed settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<WebCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llama_cpp: Option<LlamaCppCapabilities>,
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
        if key == "mcp.enabled"
            && let Some(enabled) = self.mcp.as_ref().and_then(|mcp| mcp.enabled)
        {
            return Some(enabled);
        }
        if key == "llama_cpp.enabled"
            && let Some(enabled) = self.llama_cpp.as_ref().and_then(|local| local.enabled)
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
        } else if key == "mcp.enabled" {
            self.overrides.remove(key);
            self.mcp.get_or_insert_default().enabled = Some(enabled);
            return;
        } else if key == "llama_cpp.enabled" {
            self.overrides.remove(key);
            self.llama_cpp.get_or_insert_default().enabled = Some(enabled);
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
        self.overrides.is_empty()
            && self.web.is_none()
            && self.mcp.is_none()
            && self.llama_cpp.is_none()
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
            if let Some(mcp) = &settings.mcp {
                if mcp.enabled.is_some() && settings.overrides.contains_key("mcp.enabled") {
                    return Err("mcp.enabled is configured both as a key and an mcp setting".into());
                }
                validate_mcp(&mcp.settings, role)?;
            }
            if let Some(local) = &settings.llama_cpp {
                if local.enabled.is_some() && settings.overrides.contains_key("llama_cpp.enabled") {
                    return Err(
                        "llama_cpp.enabled is configured both as a key and a llama_cpp setting"
                            .into(),
                    );
                }
                validate_llama_cpp(&local.settings)?;
            }
            if settings.get("llama_cpp.enabled") == Some(true)
                && settings
                    .llama_cpp
                    .as_ref()
                    .and_then(|local| local.settings.endpoint.as_deref())
                    .is_none_or(str::is_empty)
            {
                return Err(format!(
                    "[agent.capabilities.{}.llama_cpp] endpoint is required when enabled",
                    role.as_str()
                ));
            }
            let entries = settings
                .overrides
                .iter()
                .map(|(key, setting)| (key.as_str(), Some(setting)))
                .chain(settings.web.as_ref().map(|_| ("web.enabled", None)))
                .chain(settings.mcp.as_ref().map(|_| ("mcp.enabled", None)))
                .chain(
                    settings
                        .llama_cpp
                        .as_ref()
                        .map(|_| ("llama_cpp.enabled", None)),
                );
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
        let mcp = overrides
            .mcp
            .as_ref()
            .map_or_else(McpSettings::default, |mcp| mcp.settings.clone());
        let llama_cpp = overrides
            .llama_cpp
            .as_ref()
            .map(|local| local.settings.clone());
        CapabilitySnapshot::with_settings(
            role,
            self.0.preset.snapshot(),
            features,
            Some(web),
            Some(mcp),
            trust,
            llama_cpp,
        )
    }

    pub fn todo(&self, role: AgentRole) -> bool {
        self.resolve(role, None).enabled("todo")
    }
}

fn validate_mcp(mcp: &McpSettings, role: AgentRole) -> Result<(), String> {
    let write_proxy = regex::Regex::new(
        r"(?i)(^|[_./-])(bash|shell|exec|execute|run_command|write|edit|delete|remove|create|patch)([_./-]|$)",
    ).map_err(|error| format!("compiling MCP tool policy: {error}"))?;
    for (name, server) in &mcp.servers {
        let fail = |reason: &str| format!("mcp.servers.{name}: {reason}");
        if name.is_empty()
            || name.len() > 40
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(fail("server name must match [A-Za-z0-9_-]{1,40}"));
        }
        if server.projects.is_empty()
            || server
                .projects
                .iter()
                .any(|project| project.trim().is_empty() || project.contains(['*', '?']))
        {
            return Err(fail(
                "projects must be exact nonempty project names, without wildcards",
            ));
        }
        match (&server.command, &server.url) {
            (Some(command), None) if !command.trim().is_empty() => {}
            (None, Some(url)) if server.args.is_empty() => {
                validate_http_endpoint(url).map_err(|error| fail(&error))?;
            }
            _ => {
                return Err(fail(
                    "configure exactly one command/args or HTTP(S) url transport",
                ));
            }
        }
        if role != AgentRole::Worker && server.tools.values().any(|v| *v == McpToolAccess::Write) {
            return Err(fail("write tools are not available to this read-only role"));
        }
        for tool in server.tools.keys() {
            if tool.is_empty()
                || tool
                    .split(['_', '.', '/', '-'])
                    .any(|token| token.eq_ignore_ascii_case("corral"))
            {
                return Err(fail(
                    "tool names must be nonempty and cannot contain a corral token",
                ));
            }
            if role != AgentRole::Worker && write_proxy.is_match(tool) {
                return Err(fail(
                    "execution/write proxy tools are not available to this read-only role",
                ));
            }
        }
        if server.resources.iter().any(String::is_empty) {
            return Err(fail("resource scopes must not be empty"));
        }
    }
    Ok(())
}

fn validate_llama_cpp(local: &LlamaCppSettings) -> Result<(), String> {
    if local.context_window == 0 || local.max_tokens == 0 || local.max_tokens > local.context_window
    {
        return Err(
            "llama_cpp limits must be positive and max_tokens must not exceed context_window"
                .into(),
        );
    }
    if local.timeout_ms == 0 || local.timeout_ms > 2_147_483_647 {
        return Err("llama_cpp.timeout_ms must be between 1 and 2147483647".into());
    }
    if let Some(url) = &local.endpoint {
        let origin =
            regex::Regex::new(r"^https?://([A-Za-z0-9.-]+|\[[0-9A-Fa-f:.]+\])(?::([0-9]+))?/?$")
                .map_err(|error| format!("compiling llama_cpp endpoint pattern: {error}"))?;
        let captures = origin
            .captures(url)
            .ok_or("llama_cpp.endpoint must be an HTTP(S) origin without userinfo, path, query or fragment")?;
        if let Some(port) = captures.get(2)
            && port.as_str().parse::<u16>().is_err()
        {
            return Err("llama_cpp.endpoint has an invalid port".into());
        }
        if let Some(host) = captures.get(1) {
            let host = host.as_str();
            let valid = if let Some(ipv6) = host.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
            {
                ipv6.parse::<std::net::Ipv6Addr>().is_ok()
            } else if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
                host.parse::<std::net::Ipv4Addr>().is_ok()
            } else {
                let host = host.strip_suffix('.').unwrap_or(host);
                host.len() <= 253
                    && host.split('.').all(|label| {
                        !label.is_empty()
                            && label.len() <= 63
                            && !label.starts_with('-')
                            && !label.ends_with('-')
                    })
            };
            if !valid {
                return Err("llama_cpp.endpoint has an invalid host".into());
            }
        }
    }
    Ok(())
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

pub(crate) fn validate_http_endpoint(url: &str) -> Result<(), String> {
    let endpoint = regex::Regex::new(
            r"^https?://((?:[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)*|\[[0-9A-Fa-f:]+\])(?::([0-9]+))?(?:[/?][^\s#\\]*)?$",
        ).map_err(|error| format!("compiling HTTP endpoint pattern: {error}"))?;
    let captures = endpoint
        .captures(url)
        .ok_or("must be an HTTP(S) endpoint without userinfo or fragments")?;
    if let Some(port) = captures.get(2)
        && port.as_str().parse::<u16>().is_err()
    {
        return Err("has an invalid port".into());
    }
    if let Some(host) = captures.get(1)
        && host.as_str().starts_with('[')
        && host.as_str()[1..host.as_str().len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .is_err()
    {
        return Err("has an invalid IPv6 host".into());
    }
    if url.chars().any(char::is_control)
        || url.as_bytes().iter().enumerate().any(|(i, byte)| {
            *byte == b'%'
                && !url
                    .as_bytes()
                    .get(i + 1..i + 3)
                    .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit))
        })
    {
        return Err("has invalid URL characters or percent encoding".into());
    }
    if let Some(host) = captures.get(1)
        && !host.as_str().starts_with('[')
        && (host.as_str().len() > 253
            || host.as_str().split('.').any(|label| label.len() > 63)
            || (host
                .as_str()
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.')
                && host.as_str().parse::<std::net::Ipv4Addr>().is_err()))
    {
        return Err("has an invalid host".into());
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
fn user_dirs(table: &CapabilitySettings, defaults: &[&str]) -> Result<CapabilitySettings, String> {
    let mut dirs: Vec<_> = defaults
        .iter()
        .map(|dir| serde_json::Value::from(*dir))
        .collect();
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

/// `max_jobs`, `max_output_bytes` and `max_runtime_secs`; every bound is recorded effective.
pub(crate) fn job_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let object = serde_json::Value::Object(table.clone().into_iter().collect());
    let settings: crate::agent::JobSettings = serde_json::from_value(object).map_err(|error| {
        format!(
            "has invalid settings ({error}); known: max_jobs, max_output_bytes, max_runtime_secs"
        )
    })?;
    settings.validate()?;
    match serde_json::to_value(&settings) {
        Ok(serde_json::Value::Object(map)) => Ok(map.into_iter().collect()),
        _ => Err("background_shell settings did not serialize as a table".into()),
    }
}

pub(crate) fn skill_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    user_dirs(table, &["~/.agents/skills", "~/.corral/skills/pi-native"])
}

pub(crate) fn template_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    user_dirs(table, &["~/.agents/prompts"])
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

#[cfg(test)]
mod mcp_config_tests {
    use super::*;

    #[test]
    fn mcp_and_tool_search_are_independent_for_every_role_and_preset() {
        // Given every role and preset, including PM.
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            for role in ROLES {
                let mut config = AgentCapabilities::default();
                config.set_preset(preset);
                config.set(role, "codemode", false).unwrap();
                let default = preset != Preset::Strict;
                assert_eq!(config.resolve(role, None).enabled("mcp.enabled"), default);
                assert_eq!(config.resolve(role, None).enabled("tool_search"), default);
                // When either boolean is independently overridden.
                config.set(role, "mcp.enabled", !default).unwrap();
                // Then the other boolean is unchanged; MCP never implies codemode.
                let snapshot = config.resolve(role, None);
                assert_eq!(snapshot.enabled("mcp.enabled"), !default);
                assert_eq!(snapshot.enabled("tool_search"), default);
                assert!(!snapshot.enabled("codemode"));
                config.set(role, "tool_search", !default).unwrap();
                config.set(role, "mcp.enabled", default).unwrap();
                let snapshot = config.resolve(role, None);
                assert_eq!(snapshot.enabled("tool_search"), !default);
                assert_eq!(snapshot.enabled("mcp.enabled"), default);
            }
        }
    }

    #[test]
    fn codemode_defaults_and_overrides_are_independent_of_mcp_and_search() {
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            for role in ROLES {
                let mut config = AgentCapabilities::default();
                config.set_preset(preset);
                let enabled =
                    preset == Preset::Full || (preset == Preset::Standard && role != AgentRole::Pm);
                assert_eq!(config.resolve(role, None).enabled("codemode"), enabled);
                for key in ["mcp.enabled", "tool_search"] {
                    config.set(role, key, false).unwrap();
                }
                assert_eq!(config.resolve(role, None).enabled("codemode"), enabled);
                config.set(role, "codemode", !enabled).unwrap();
                assert_eq!(config.resolve(role, None).enabled("codemode"), !enabled);
            }
        }
        let config: AgentCapabilities = toml::from_str("[pm]\ncodemode = true").unwrap();
        assert!(config.resolve(AgentRole::Pm, None).enabled("codemode"));
    }

    #[test]
    fn mcp_transports_and_project_policy_parse_with_defaults_and_round_trip() {
        // Given both supported transport forms and explicit project enablement.
        for transport in ["command = 'docs-mcp'", "url = 'http://[::1]:8080/mcp'"] {
            let text = format!(
                "[worker.mcp.servers.docs]\nprojects = ['corral']\n\
                 tools = {{ search = 'read' }}\n{transport}"
            );
            // When parsing the workspace policy.
            let config: AgentCapabilities = toml::from_str(&text).unwrap();
            let encoded = toml::to_string(&config).unwrap();
            // Then absent args/resources default empty and explicit bindings survive.
            assert_eq!(
                toml::from_str::<AgentCapabilities>(&encoded).unwrap(),
                config
            );
            let server = &config.resolve(AgentRole::Worker, None).mcp.unwrap().servers["docs"];
            assert_eq!(server.projects, ["corral"]);
            assert!(server.args.is_empty() && server.resources.is_empty());
        }
    }

    #[test]
    fn mcp_rejects_invalid_transports_secrets_and_project_wildcards() {
        // Given invalid server settings at the configuration boundary.
        for settings in [
            "command = ''",
            "url = 'ftp://example.org'",
            "url = 'https://user:pass@example.org'",
            "url = 'https://example.org/#fragment'",
            "url = 'https://example..org'",
            "url = 'http://[:::1]'",
            "url = 'http://localhost:99999'",
            "url = 'https://example.org/%ZZ'",
            "url = 'http://999.1.1.1'",
            "url = 'http://example.org'\nargs = ['--stdio']",
            "command = 'mcp'\nurl = 'https://example.org'",
            "args = []",
            "command = 'mcp'\nenv = { TOKEN = 'secret' }",
            "command = 'mcp'\nheaders = { Authorization = 'secret' }",
            "command = 'mcp'\nunknown = true",
        ] {
            // When parsing, then malformed or ambiguous transport and secret fields are rejected.
            assert!(
                toml::from_str::<AgentCapabilities>(&format!(
                    "[worker.mcp.servers.docs]\nprojects = ['corral']\ntools = {{}}\n{settings}"
                ))
                .is_err(),
                "{settings}"
            );
        }
        for projects in [
            "",
            "projects = []",
            "projects = ['*']",
            "projects = ['cor?al']",
            "projects = ['']",
        ] {
            assert!(
                toml::from_str::<AgentCapabilities>(&format!(
                    "[worker.mcp.servers.docs]\ncommand = 'mcp'\ntools = {{}}\n{projects}"
                ))
                .is_err(),
                "{projects}"
            );
        }
        assert!(
            toml::from_str::<AgentCapabilities>(
                "[worker]\n'mcp.enabled' = true\n[worker.mcp]\nenabled = false"
            )
            .is_err()
        );
    }

    #[test]
    fn mcp_write_policy_is_refused_for_all_read_only_roles_even_when_disabled() {
        // Given a server whose configured ceiling permits writes.
        for role in ROLES {
            // When parsing each role, then only workers may retain this policy.
            let text = format!(
                "[{}.mcp]\nenabled = false\n[{}.mcp.servers.docs]\n\
                 projects = ['corral']\ncommand = 'mcp'\ntools = {{ edit = 'write' }}",
                role.as_str(),
                role.as_str()
            );
            assert_eq!(
                toml::from_str::<AgentCapabilities>(&text).is_ok(),
                role == AgentRole::Worker
            );
        }
    }
}

#[cfg(test)]
mod llama_cpp_tests {
    use super::*;

    #[test]
    fn llama_cpp_requires_explicit_opt_in_for_every_role_and_preset() {
        // Given each role and preset, with no local provider configuration.
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            let mut config = AgentCapabilities::default();
            config.set_preset(preset);
            for role in ROLES {
                // When resolved, then even full and legacy leave local access off.
                let snapshot = config.resolve(role, None);
                assert!(!snapshot.enabled("llama_cpp.enabled"));
                assert!(snapshot.llama_cpp.is_none());
                assert!(!CapabilitySnapshot::legacy(role, true).enabled("llama_cpp.enabled"));
                // A boolean-only opt-in fails atomically, just as parsing does.
                assert!(config.set(role, "llama_cpp.enabled", true).is_err());
                assert!(!config.resolve(role, None).enabled("llama_cpp.enabled"));
            }
        }
    }

    #[test]
    fn llama_cpp_opt_in_round_trips_and_preserves_dotted_setters() {
        // Given an explicit endpoint for each allowed role.
        for role in ROLES {
            let text = format!(
                "[{}.llama_cpp]\nenabled = true\nendpoint = 'http://[::1]:8080/'",
                role.as_str()
            );
            // When parsed and round-tripped, then all defaults and the opt-in survive.
            let mut config: AgentCapabilities = toml::from_str(&text).unwrap();
            let encoded = toml::to_string(&config).unwrap();
            assert_eq!(
                toml::from_str::<AgentCapabilities>(&encoded).unwrap(),
                config
            );
            let snapshot = config.resolve(role, None);
            assert!(snapshot.enabled("llama_cpp.enabled"));
            assert_eq!(snapshot.catalog, 4);
            assert_eq!(
                snapshot.llama_cpp.unwrap(),
                LlamaCppSettings {
                    endpoint: Some("http://[::1]:8080/".into()),
                    ..LlamaCppSettings::default()
                }
            );
            config.set(role, "llama_cpp.enabled", false).unwrap();
            assert!(config.resolve(role, None).llama_cpp.is_none());
            config.set(role, "llama_cpp.enabled", true).unwrap();
            assert!(
                config
                    .resolve(role, Some(&BTreeSet::new()))
                    .llama_cpp
                    .is_none()
            );
        }
        let mut role: RoleCapabilities = toml::from_str("\"llama_cpp.enabled\" = false").unwrap();
        assert_eq!(role.get("llama_cpp.enabled"), Some(false));
        role.set("llama_cpp.enabled", true);
        assert_eq!(role.get("llama_cpp.enabled"), Some(true));
        let encoded = toml::to_string(&role).unwrap();
        assert_eq!(toml::from_str::<RoleCapabilities>(&encoded).unwrap(), role);
        let dotted =
            "\"llama_cpp.enabled\" = true\n[worker.llama_cpp]\nendpoint = 'http://localhost:8080'";
        assert!(toml::from_str::<AgentCapabilities>(&format!("[worker]\n{dotted}")).is_ok());
    }

    #[test]
    fn llama_cpp_rejects_invalid_origins_and_limits_at_parse_boundary() {
        // Given malformed origins and out-of-contract limits.
        for setting in [
            "enabled = true",
            "\"llama_cpp.enabled\" = true",
            "endpoint = ''",
            "endpoint = 'ftp://localhost'",
            "endpoint = 'http://user@localhost'",
            "endpoint = 'http://localhost/path'",
            "endpoint = 'http://localhost?x=1'",
            "endpoint = 'http://localhost#x'",
            "endpoint = 'http://local host'",
            "endpoint = 'http://localhost\\path'",
            "endpoint = 'http://[:::1]'",
            "endpoint = 'http://[::1]:65536'",
            "endpoint = 'http://-host'",
            "endpoint = 'http://host..test'",
            "endpoint = 'http://999.0.0.1'",
            "context_window = 0",
            "max_tokens = 0",
            "context_window = 1",
            "timeout_ms = 0",
            "timeout_ms = 2147483648",
            "unknown = true",
        ] {
            // When parsed, then malformed settings cannot reach a snapshot.
            assert!(
                toml::from_str::<AgentCapabilities>(&format!("[worker.llama_cpp]\n{setting}"))
                    .is_err(),
                "{setting}"
            );
        }
        assert!(
            toml::from_str::<AgentCapabilities>("[worker]\n\"llama_cpp.enabled\" = true").is_err()
        );
        for endpoint in [
            "http://localhost",
            "https://host.test:443/",
            "http://127.0.0.1:8080",
            "http://[::ffff:127.0.0.1]:8080",
        ] {
            let text = format!(
                "[worker.llama_cpp]\nenabled = true\nendpoint = '{endpoint}'\n\
                 context_window = 1\nmax_tokens = 1\ntimeout_ms = 2147483647\ndiscover = false"
            );
            assert!(
                toml::from_str::<AgentCapabilities>(&text).is_ok(),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn llama_cpp_settings_change_digest_and_record_concrete_transition() {
        // Given a local policy and independent changes to each setting.
        let text = "[worker.llama_cpp]\nenabled = true\nendpoint = 'http://localhost:8080'\n";
        let original = toml::from_str::<AgentCapabilities>(text)
            .unwrap()
            .resolve(AgentRole::Worker, None);
        for change in [
            "discover = false",
            "context_window = 65536",
            "max_tokens = 4096",
            "timeout_ms = 30000",
            "endpoint = 'https://host.test/'",
        ] {
            let base = if change.starts_with("endpoint") {
                "[worker.llama_cpp]\nenabled = true\n"
            } else {
                text
            };
            // When resolved, then the digest and concrete settings diff track the change.
            let config: AgentCapabilities = toml::from_str(&format!("{base}{change}")).unwrap();
            let next = config.resolve(AgentRole::Worker, None);
            assert_eq!(next, config.resolve(AgentRole::Worker, None));
            assert_ne!(next.digest, original.digest);
            let transition = next.transition_from(1, &original).unwrap();
            assert!(transition.changes.is_empty());
            assert!(transition.web_change.is_none());
            let local = transition.llama_cpp_change.unwrap();
            assert_eq!(local.from, original.llama_cpp);
            assert_eq!(local.to, next.llama_cpp);
        }
    }
}
