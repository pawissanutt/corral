//! `corral-extension.toml` v1 (docs/extensions-protocol.md, "Manifest").

use super::envelope::{InitializeParams, InitializeResult, capability_id};
use super::error::{ErrorCode, ExtError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

pub const MANIFEST_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "corral-extension.toml";
pub const PUBLIC_READS: [&str; 3] = ["read.batch", "read.changes", "read.snapshot"];
pub const SUBSCRIPTION_SCOPES: [&str; 3] = ["task", "lessons", "artifact"];
pub const SUBSCRIPTION_EVENTS: [&str; 2] = ["created", "moved"];

const TOP_KEYS: [&str; 12] = [
    "manifest_version",
    "id",
    "version",
    "executable",
    "protocol_versions",
    "execution_modes",
    "entry",
    "concurrency",
    "providers",
    "config_schema",
    "subscriptions",
    "requested_actions",
];
const TOP_REQUIRED: [&str; 6] = [
    "manifest_version",
    "id",
    "version",
    "executable",
    "protocol_versions",
    "execution_modes",
];
const PROVIDER_KEYS: [&str; 3] = ["name", "versions", "reads"];
const SUBSCRIPTION_KEYS: [&str; 3] = ["name", "cursor_kind", "filter"];
const ACTION_KEYS: [&str; 1] = ["action"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    pub id: String,
    pub version: String,
    pub executable: String,
    pub protocol_versions: Vec<u32>,
    pub execution_modes: Vec<ExecutionMode>,
    #[serde(default)]
    pub entry: Vec<String>,
    #[serde(default = "one")]
    pub concurrency: u32,
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default)]
    pub config_schema: toml::Table,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub requested_actions: Vec<RequestedAction>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Oneshot,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub name: String,
    pub versions: Vec<u32>,
    #[serde(default)]
    pub reads: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    ChangeSeq,
    EventSeq,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub name: String,
    pub cursor_kind: CursorKind,
    #[serde(default)]
    pub filter: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestedAction {
    pub action: String,
}

/// A manifest rejection naming the offending field (`providers[0].reads`, `id`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub field: String,
    pub reason: String,
}

impl ManifestError {
    fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }

    pub fn to_ext_error(&self) -> ExtError {
        ExtError::invalid_request(self.to_string())
            .with_data(serde_json::json!({ "field": self.field }))
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "manifest field `{}`: {}", self.field, self.reason)
    }
}

impl std::error::Error for ManifestError {}

/// A declared feature that parses in v1 but cannot be activated until its stage lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotActivatable {
    pub field: &'static str,
    pub stage: u8,
}

impl Manifest {
    /// Parses and validates manifest text. Unknown and missing keys are reported by path
    /// before typed decoding, so the error always names the field.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| ManifestError::new("manifest", e.message()))?;
        check_keys(&table, "", &TOP_KEYS, &TOP_REQUIRED)?;
        check_tables(&table, "providers", &PROVIDER_KEYS, &["name", "versions"])?;
        check_tables(
            &table,
            "subscriptions",
            &SUBSCRIPTION_KEYS,
            &["name", "cursor_kind"],
        )?;
        check_tables(&table, "requested_actions", &ACTION_KEYS, &["action"])?;
        let manifest: Self = toml::from_str(text).map_err(|e| {
            let field = e
                .span()
                .and_then(|span| key_at(text, span.start))
                .unwrap_or_else(|| "manifest".into());
            ManifestError::new(field, e.message())
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(ManifestError::new(
                "manifest_version",
                format!("must be {MANIFEST_VERSION}"),
            ));
        }
        if !is_extension_id(&self.id) {
            return Err(ManifestError::new(
                "id",
                "must match ^[a-z][a-z0-9-]{0,31}$",
            ));
        }
        if !is_semver(&self.version) {
            return Err(ManifestError::new("version", "must be a semver version"));
        }
        if self.executable.is_empty() || self.executable.contains('\0') {
            return Err(ManifestError::new("executable", "must be a non-empty path"));
        }
        check_versions("protocol_versions", &self.protocol_versions)?;
        if self.execution_modes.is_empty() || has_duplicates(&self.execution_modes) {
            return Err(ManifestError::new(
                "execution_modes",
                "must list each mode at most once and at least one mode",
            ));
        }
        if let Some(i) = self.entry.iter().position(|a| a.contains('\0')) {
            return Err(ManifestError::new(
                format!("entry[{i}]"),
                "must not contain NUL",
            ));
        }
        if self.concurrency == 0 {
            return Err(ManifestError::new("concurrency", "must be at least 1"));
        }
        let mut names = BTreeSet::new();
        for (i, provider) in self.providers.iter().enumerate() {
            if !is_capability_name(&provider.name) {
                return Err(ManifestError::new(
                    format!("providers[{i}].name"),
                    "must be dotted lowercase segments such as task.context",
                ));
            }
            if !names.insert(provider.name.as_str()) {
                return Err(ManifestError::new(
                    format!("providers[{i}].name"),
                    "is declared twice",
                ));
            }
            check_versions(&format!("providers[{i}].versions"), &provider.versions)?;
            if let Some(j) = provider
                .reads
                .iter()
                .position(|r| !PUBLIC_READS.contains(&r.as_str()))
            {
                return Err(ManifestError::new(
                    format!("providers[{i}].reads[{j}]"),
                    format!("must be one of {}", PUBLIC_READS.join(", ")),
                ));
            }
        }
        let mut names = BTreeSet::new();
        for (i, sub) in self.subscriptions.iter().enumerate() {
            if !is_extension_id(&sub.name) || !names.insert(sub.name.as_str()) {
                return Err(ManifestError::new(
                    format!("subscriptions[{i}].name"),
                    "must be a unique lowercase name",
                ));
            }
            let allowed: &[&str] = match sub.cursor_kind {
                CursorKind::ChangeSeq => &SUBSCRIPTION_SCOPES,
                CursorKind::EventSeq => &SUBSCRIPTION_EVENTS,
            };
            if let Some(j) = sub
                .filter
                .iter()
                .position(|f| !allowed.contains(&f.as_str()))
            {
                return Err(ManifestError::new(
                    format!("subscriptions[{i}].filter[{j}]"),
                    format!("must be one of {}", allowed.join(", ")),
                ));
            }
        }
        for (i, action) in self.requested_actions.iter().enumerate() {
            if !is_capability_name(&action.action) {
                return Err(ManifestError::new(
                    format!("requested_actions[{i}].action"),
                    "must be dotted lowercase segments",
                ));
            }
        }
        Ok(())
    }

    /// Declared features v1 parses but refuses to activate: subscriptions (stage 2,
    /// durable invalidation) and requested actions (stage 3, grants).
    pub fn activation_blockers(&self) -> Vec<NotActivatable> {
        let mut blockers = Vec::new();
        if !self.subscriptions.is_empty() {
            blockers.push(NotActivatable {
                field: "subscriptions",
                stage: 2,
            });
        }
        if !self.requested_actions.is_empty() {
            blockers.push(NotActivatable {
                field: "requested_actions",
                stage: 3,
            });
        }
        blockers
    }

    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Checks an `initialize` result against the host's offer and this manifest. A
    /// capability the manifest does not declare is `manifest_mismatch`, whatever was offered.
    pub fn check_initialize(
        &self,
        offer: &InitializeParams,
        result: &InitializeResult,
    ) -> Result<(), ExtError> {
        let version = result.protocol_version;
        if !offer.protocol_versions.contains(&version) || !self.protocol_versions.contains(&version)
        {
            return Err(ExtError::new(
                ErrorCode::UnsupportedProtocol,
                format!("protocol version {version} was not offered or not declared"),
            ));
        }
        let mut seen = BTreeSet::new();
        for choice in &result.capabilities {
            let id = capability_id(&choice.name, choice.version);
            if !seen.insert(choice.name.as_str()) {
                return Err(ExtError::protocol_violation(
                    "duplicate_capability",
                    format!("capability {} is chosen twice", choice.name),
                ));
            }
            let declared = self
                .provider(&choice.name)
                .is_some_and(|p| p.versions.contains(&choice.version));
            if !declared {
                return Err(ExtError::new(
                    ErrorCode::ManifestMismatch,
                    format!("capability {id} is not declared in the manifest"),
                )
                .with_data(serde_json::json!({ "capability": id })));
            }
            let offered = offer
                .capabilities
                .iter()
                .any(|c| c.name == choice.name && c.versions.contains(&choice.version));
            if !offered {
                return Err(ExtError::new(
                    ErrorCode::UnsupportedCapability,
                    format!("capability {id} was not offered"),
                ));
            }
        }
        Ok(())
    }
}

fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

fn check_keys(
    table: &toml::Table,
    prefix: &str,
    known: &[&str],
    required: &[&str],
) -> Result<(), ManifestError> {
    if let Some(key) = table.keys().find(|k| !known.contains(&k.as_str())) {
        return Err(ManifestError::new(join(prefix, key), "unknown key"));
    }
    if let Some(key) = required.iter().find(|k| !table.contains_key(**k)) {
        return Err(ManifestError::new(
            join(prefix, key),
            "required key is missing",
        ));
    }
    Ok(())
}

fn check_tables(
    table: &toml::Table,
    key: &str,
    known: &[&str],
    required: &[&str],
) -> Result<(), ManifestError> {
    let items = match table.get(key) {
        None => return Ok(()),
        Some(toml::Value::Array(items)) => items,
        Some(_) => return Err(ManifestError::new(key, "must be an array of tables")),
    };
    for (i, item) in items.iter().enumerate() {
        let path = format!("{key}[{i}]");
        let toml::Value::Table(entry) = item else {
            return Err(ManifestError::new(path, "must be a table"));
        };
        check_keys(entry, &path, known, required)?;
    }
    Ok(())
}

/// The key assigned on the line containing `offset`, for typed-decoding errors.
fn key_at(text: &str, offset: usize) -> Option<String> {
    let start = text.get(..offset)?.rfind('\n').map_or(0, |i| i + 1);
    let line = text.get(start..)?.lines().next()?;
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

fn check_versions(field: &str, versions: &[u32]) -> Result<(), ManifestError> {
    if versions.is_empty() || versions.contains(&0) || has_duplicates(versions) {
        return Err(ManifestError::new(
            field,
            "must list distinct versions of at least 1",
        ));
    }
    Ok(())
}

fn has_duplicates<T: Eq + std::hash::Hash>(items: &[T]) -> bool {
    let mut seen = std::collections::HashSet::new();
    !items.iter().all(|item| seen.insert(item))
}

/// `^[a-z][a-z0-9-]{0,31}$`
pub fn is_extension_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn is_capability_name(name: &str) -> bool {
    name.len() <= 64
        && name.split('.').all(|segment| {
            let bytes = segment.as_bytes();
            !bytes.is_empty()
                && bytes[0].is_ascii_lowercase()
                && bytes[1..]
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
        })
}

fn is_semver(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let rest = &version[core.len()..];
    let numeric = core.split('.').collect::<Vec<_>>();
    numeric.len() == 3
        && numeric.iter().all(|n| {
            !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && (n.len() == 1 || !n.starts_with('0'))
        })
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-+.".contains(&b))
        && !rest.ends_with(['-', '+', '.'])
}
