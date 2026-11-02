//! Assistant write permissions (T-656 grant-store): the owner's scoped, revocable permission for
//! the assistant to edit or write outside its default roots. The host classifies paths and shows
//! the dialog; corral keeps the decision records, the grants and their audit outbox on the
//! session's container and decides every grant check.
//!
//! Every path here is a `{shown, canonical}` pair: the lexical form the owner saw (absolute or
//! `~/`-relative) and its symlink-resolved form. A grant matches only by canonical containment,
//! and only while its shown scope still resolves to the recorded canonical scope.
use std::path::{Component, Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::capabilities::CapabilitySettings;
use super::paths::sha256_hex;
use crate::assistant::{AssistantContainer, NewOwnerInput, OwnerInput, OwnerInputSource};

/// Grant records a session keeps; inactive ones are evicted first, active ones never.
pub const WRITE_GRANTS_MAX: usize = 64;
/// Permission entries the owner-input journal keeps, so they never crowd out citable inputs.
pub const PERMISSION_INPUTS_MAX: usize = 32;
/// Entries in each of the `allow` and `deny` settings lists.
pub const SETTINGS_ENTRIES_MAX: usize = 64;
/// Bytes of one path in a setting, a decision or a grant.
pub const PERMISSION_PATH_MAX: usize = 4096;
/// Bytes of a permission entry's one-line journal summary.
pub const PERMISSION_TEXT_MAX: usize = 1024;
/// Bytes of the transcript tool-call ID an invocation names.
pub const TOOL_CALL_MAX: usize = 256;
/// How long an hour grant lasts from corral's wall clock.
pub const HOUR_GRANT: SignedDuration = SignedDuration::from_secs(3600);
/// Path components no grant, setting or default root can cover.
pub const PROTECTED_COMPONENTS: [&str; 3] = [".corral", ".git", ".worktrees"];

/// The home directory the scope limit protects, in its lexical and canonical forms.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Home {
    pub lexical: Option<PathBuf>,
    pub canonical: Option<PathBuf>,
}

impl Home {
    /// `path` normalized, and its resolved form when it exists; a relative path is no home.
    pub fn new(path: impl AsRef<Path>) -> Self {
        let Ok(lexical) = normalize(path.as_ref()) else {
            return Self::default();
        };
        let canonical = std::fs::canonicalize(&lexical).ok();
        Self {
            lexical: Some(lexical),
            canonical,
        }
    }

    /// `$HOME`, as elsewhere in corral; unset or empty means no home is known.
    pub fn from_env() -> Self {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(Self::new)
            .unwrap_or_default()
    }

    fn dirs(&self) -> impl Iterator<Item = &Path> {
        self.lexical
            .iter()
            .chain(&self.canonical)
            .map(PathBuf::as_path)
    }
}

/// `path` with `.`, repeated and trailing separators removed. It must be absolute; `..` is
/// refused outright rather than resolved.
pub fn normalize(path: &Path) -> Result<PathBuf, String> {
    if !path.has_root() {
        return Err("must be an absolute path".into());
    }
    let mut out = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir => return Err("must not contain '..'".into()),
            Component::Prefix(_) => return Err("must be an absolute path".into()),
        }
    }
    Ok(out)
}

/// An absolute, `~` or `~/...` path of 1-4096 bytes, expanded against `home` and normalized.
pub fn expand(path: &str, home: &Home) -> Result<PathBuf, String> {
    if path.is_empty() || path.len() > PERMISSION_PATH_MAX || path.contains('\0') {
        return Err(format!("must be 1-{PERMISSION_PATH_MAX} bytes without NUL"));
    }
    let full = if path == "~" || path.starts_with("~/") {
        let home = home
            .lexical
            .as_deref()
            .ok_or("names ~, but HOME is not set")?;
        home.join(path.trim_start_matches('~').trim_start_matches('/'))
    } else if path.starts_with('/') {
        PathBuf::from(path)
    } else {
        return Err("must be absolute or start with ~/".into());
    };
    normalize(&full)
}

/// The scope limit: a grant scope or allow entry is never `/`, a top-level directory, or the
/// home directory or an ancestor of it (lexical or canonical home). `path` is a normalized or
/// canonical form; callers check every form they hold.
pub fn scope_limit(path: &Path, home: &Home) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("is not an absolute path".into());
    }
    let mut normals = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => normals += 1,
            Component::ParentDir => return Err("must not contain '..'".into()),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    match normals {
        0 => Err("is the root directory".into()),
        1 => Err("is a top-level directory".into()),
        _ if home.dirs().any(|dir| dir.starts_with(path)) => {
            Err("is the home directory or an ancestor of it".into())
        }
        _ => Ok(()),
    }
}

/// The first `.corral`, `.git` or `.worktrees` component of `path`, if any.
pub fn protected_component(path: &Path) -> Option<&'static str> {
    path.components().find_map(|component| match component {
        Component::Normal(part) => PROTECTED_COMPONENTS
            .iter()
            .find(|name| part == **name)
            .copied(),
        _ => None,
    })
}

/// `[agent.capabilities.assistant.write_permissions]`: directory prefixes written with no
/// question (`allow`) and always refused (`deny`). Deny beats allow and every grant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WritePermissionSettings {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

/// One settings entry's syntax: an absolute or `~/` prefix with no glob character or `..`.
pub fn setting_path(entry: &str, home: &Home) -> Result<PathBuf, String> {
    if entry.contains(['*', '?', '[']) {
        return Err("must be a directory prefix, not a glob".into());
    }
    expand(entry, home)
}

impl WritePermissionSettings {
    /// Both lists' syntax and size; allow entries must also pass the scope limit in their
    /// expanded, normalized form. Deny entries may be broad: they only remove authority.
    pub fn validate(&self, home: &Home) -> Result<(), String> {
        for (list, entries, limited) in [("allow", &self.allow, true), ("deny", &self.deny, false)]
        {
            if entries.len() > SETTINGS_ENTRIES_MAX {
                return Err(format!(
                    "write_permissions.{list} lists more than {SETTINGS_ENTRIES_MAX} entries"
                ));
            }
            for entry in entries {
                let named = |why: String| format!("write_permissions.{list} entry {entry:?} {why}");
                let path = setting_path(entry, home).map_err(named)?;
                if limited {
                    scope_limit(&path, home).map_err(named)?;
                }
            }
        }
        Ok(())
    }
}

/// The catalog's settings function for `write_permissions`, with HOME from the environment.
pub fn write_permission_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let settings: WritePermissionSettings =
        serde_json::from_value(Value::Object(table.clone().into_iter().collect()))
            .map_err(|error| format!("is invalid: {error}"))?;
    settings.validate(&Home::from_env())?;
    match serde_json::to_value(&settings) {
        Ok(Value::Object(map)) => Ok(map.into_iter().collect()),
        _ => Err("could not be recorded".into()),
    }
}

/// An allow entry's current canonical form, or why it is ignored: bad syntax, a broad lexical
/// form, no resolution, or a resolution that is broad (for example a symlink relinked to `/`).
pub fn resolve_allow(
    entry: &str,
    home: &Home,
    resolve: impl Fn(&Path) -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    let lexical = setting_path(entry, home)?;
    scope_limit(&lexical, home)?;
    let canonical = resolve(&lexical).ok_or("does not resolve to an existing directory")?;
    scope_limit(&canonical, home)
        .map_err(|why| format!("resolves to {}, which {why}", canonical.display()))?;
    Ok(canonical)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteTool {
    Edit,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    NewFile,
    Overwrite,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteOperation {
    pub kind: OperationKind,
    /// An edit's replacement count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacements: Option<u32>,
}

impl WriteOperation {
    /// write is a new file or an overwrite; edit is an edit with at least one replacement.
    pub fn fits(&self, tool: WriteTool) -> bool {
        match (tool, self.kind, self.replacements) {
            (WriteTool::Write, OperationKind::NewFile | OperationKind::Overwrite, None) => true,
            (WriteTool::Edit, OperationKind::Edit, replacements) => replacements != Some(0),
            _ => false,
        }
    }
}

/// A path as the owner saw it and as it resolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionPath {
    pub shown: String,
    pub canonical: String,
}

/// One live tool invocation that reached the dialog: a fresh random ID per invocation, never
/// the transcript's tool call, which a resumed generation can repeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub generation: u32,
    /// 32 lowercase hex digits.
    pub id: String,
    pub tool_call: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionChoice {
    Once,
    Hour,
    Session,
    Deny,
}

impl PermissionChoice {
    pub fn lifetime(self) -> Option<GrantLifetime> {
        match self {
            PermissionChoice::Once => Some(GrantLifetime::Once),
            PermissionChoice::Hour => Some(GrantLifetime::Hour),
            PermissionChoice::Session => Some(GrantLifetime::Session),
            PermissionChoice::Deny => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PermissionChoice::Once => "once",
            PermissionChoice::Hour => "hour",
            PermissionChoice::Session => "session",
            PermissionChoice::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantLifetime {
    Once,
    Hour,
    Session,
}

impl GrantLifetime {
    pub fn as_str(self) -> &'static str {
        match self {
            GrantLifetime::Once => "once",
            GrantLifetime::Hour => "hour",
            GrantLifetime::Session => "session",
        }
    }
}

/// PermissionDecision v1: the owner's answer as the journal entry and the never-evicted
/// `assistant_write_permission` event both record it. Deny carries no lifetime, scope or grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDecision {
    pub tool: WriteTool,
    pub operation: WriteOperation,
    pub target: PermissionPath,
    /// SHA-256 of the operation the owner was shown.
    pub digest: String,
    pub invocation: Invocation,
    pub choice: PermissionChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<GrantLifetime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PermissionPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
}

/// WriteGrant v1 on the session's container. A once grant covers exactly its target and is
/// consumed when it is made; hour and session grants cover their scope until they end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteGrant {
    /// `G-<n>`, never reused within a session.
    pub id: String,
    /// The permission journal entry that made it.
    pub input: String,
    pub generation: u32,
    pub invocation: Invocation,
    /// The decide request that made it.
    pub request: String,
    pub tool: WriteTool,
    pub operation: WriteOperation,
    pub target: PermissionPath,
    pub scope: PermissionPath,
    pub lifetime: GrantLifetime,
    pub granted_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,
    /// `owner` or the assistant session whose shell revoked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    Active,
    Consumed,
    Expired,
    Revoked,
    Ended,
}

impl GrantState {
    pub fn as_str(self) -> &'static str {
        match self {
            GrantState::Active => "active",
            GrantState::Consumed => "consumed",
            GrantState::Expired => "expired",
            GrantState::Revoked => "revoked",
            GrantState::Ended => "ended",
        }
    }
}

impl WriteGrant {
    /// An hour grant expires at its deadline. A backward wall-clock adjustment must not
    /// expire it early; it can instead extend the grant until the clock reaches its deadline.
    /// A session grant ends only with its session.
    pub fn expired(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|end| now >= end)
    }

    pub fn state(&self, now: Timestamp, ended: bool) -> GrantState {
        if self.revoked_at.is_some() {
            GrantState::Revoked
        } else if self.consumed_at.is_some() {
            GrantState::Consumed
        } else if self.expired(now) {
            GrantState::Expired
        } else if ended {
            GrantState::Ended
        } else {
            GrantState::Active
        }
    }

    pub fn active(&self, now: Timestamp) -> bool {
        self.state(now, false) == GrantState::Active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEvent {
    AssistantWritePermission,
    AssistantWritePermissionRevoked,
}

impl AuditEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditEvent::AssistantWritePermission => "assistant_write_permission",
            AuditEvent::AssistantWritePermissionRevoked => "assistant_write_permission_revoked",
        }
    }
}

/// One queued audit line: written to the event log after its transaction commits, then
/// cleared. A line repeated after a crash carries the same key, so readers count it once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionAudit {
    /// The decide request ID, or `<grant>:revoked`.
    pub key: String,
    pub event: AuditEvent,
    pub detail: Value,
}

/// The first active hour or session grant whose canonical scope holds `canonical`, whose shown
/// scope still resolves to that canonical scope and whose forms all pass the scope limit.
pub fn check<'a>(
    grants: &'a [WriteGrant],
    canonical: &Path,
    now: Timestamp,
    home: &Home,
    resolve: impl Fn(&Path) -> Option<PathBuf>,
) -> Option<&'a WriteGrant> {
    grants.iter().find(|grant| {
        let scope = Path::new(&grant.scope.canonical);
        grant.lifetime != GrantLifetime::Once
            && grant.active(now)
            && canonical.starts_with(scope)
            && scope_limit(scope, home).is_ok()
            && expand(&grant.scope.shown, home).is_ok_and(|lexical| {
                scope_limit(&lexical, home).is_ok()
                    && resolve(&lexical).is_some_and(|now_resolved| now_resolved == scope)
            })
    })
}

/// A decision corral validated, ready to journal.
#[derive(Debug, Clone)]
pub struct NewPermission {
    pub generation: u32,
    pub turn: Option<u64>,
    pub request: String,
    pub tool: WriteTool,
    pub operation: WriteOperation,
    pub target: PermissionPath,
    pub digest: String,
    pub invocation: Invocation,
    pub choice: PermissionChoice,
    pub scope: Option<PermissionPath>,
}

/// 64 active grants: an allow decision is refused and nothing changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantsFull;

fn summary(decision: &PermissionDecision) -> String {
    let operation = match decision.operation.kind {
        OperationKind::NewFile => "new_file",
        OperationKind::Overwrite => "overwrite",
        OperationKind::Edit => "edit",
    };
    let mut text = format!(
        "write permission {}: {} {operation} {}",
        decision.choice.as_str(),
        if decision.tool == WriteTool::Edit {
            "edit"
        } else {
            "write"
        },
        decision.target.shown
    );
    if let Some(scope) = &decision.scope {
        text.push_str(&format!(" scope {}", scope.shown));
    }
    if let Some(grant) = &decision.grant {
        text.push_str(&format!(" ({grant})"));
    }
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn bounded(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

impl AssistantContainer {
    /// Records one owner decision in a single container change: makes room for an allow's
    /// grant (evicting the oldest inactive record, or refusing with nothing changed), journals
    /// the `permission` entry with its structured record, stores the grant (a once grant
    /// already consumed) and queues the decision's audit line.
    pub fn record_permission(
        &mut self,
        session: &str,
        new: NewPermission,
        now: Timestamp,
    ) -> Result<(OwnerInput, Option<WriteGrant>), GrantsFull> {
        let lifetime = new.choice.lifetime();
        if lifetime.is_some() && self.write_grants.len() >= WRITE_GRANTS_MAX {
            let inactive = self
                .write_grants
                .iter()
                .position(|grant| !grant.active(now))
                .ok_or(GrantsFull)?;
            self.write_grants.remove(inactive);
        }
        let grant_id = lifetime.map(|_| {
            self.write_grants_issued += 1;
            format!("G-{}", self.write_grants_issued)
        });
        let expires_at = (lifetime == Some(GrantLifetime::Hour))
            .then(|| now.checked_add(HOUR_GRANT).ok())
            .flatten();
        let decision = PermissionDecision {
            tool: new.tool,
            operation: new.operation.clone(),
            target: new.target.clone(),
            digest: new.digest,
            invocation: new.invocation.clone(),
            choice: new.choice,
            lifetime,
            scope: new.scope.clone(),
            granted_at: lifetime.map(|_| now),
            expires_at,
            grant: grant_id.clone(),
        };
        let full = summary(&decision);
        let (text, truncated) = bounded(&full, PERMISSION_TEXT_MAX);
        let input = NewOwnerInput {
            generation: new.generation,
            turn: new.turn,
            source: OwnerInputSource::Permission,
            sha256: sha256_hex(full.as_bytes()),
            text,
            truncated,
            command: None,
            answers: None,
            request: new.request.clone(),
        };
        let entry = self.journal_permission(session, input, decision.clone());
        let grant = match (grant_id, lifetime) {
            (Some(id), Some(lifetime)) => {
                let grant = WriteGrant {
                    id,
                    input: entry.id.clone(),
                    generation: new.generation,
                    invocation: new.invocation,
                    request: new.request.clone(),
                    tool: new.tool,
                    operation: new.operation,
                    scope: new.scope.unwrap_or_else(|| new.target.clone()),
                    target: new.target,
                    lifetime,
                    granted_at: now,
                    expires_at,
                    consumed_at: (lifetime == GrantLifetime::Once).then_some(now),
                    revoked_at: None,
                    revoked_by: None,
                };
                self.write_grants.push(grant.clone());
                Some(grant)
            }
            _ => None,
        };
        self.permission_audit.push(PermissionAudit {
            key: new.request.clone(),
            event: AuditEvent::AssistantWritePermission,
            detail: json!({
                "key": new.request, "session": session, "generation": new.generation,
                "input": entry.id, "decision": decision,
            }),
        });
        Ok((entry, grant))
    }

    /// Revokes grant `which` (or every active grant) for `by`, queueing one audit line per
    /// revoked grant in the same change. Only an active grant can be revoked.
    pub fn revoke_write_grants(
        &mut self,
        session: &str,
        which: Option<&str>,
        by: &str,
        now: Timestamp,
        ended: bool,
    ) -> Result<Vec<String>, String> {
        if let Some(id) = which {
            let grant = self
                .write_grants
                .iter()
                .find(|grant| grant.id == id)
                .ok_or_else(|| format!("{session} has no write grant {id}"))?;
            let state = grant.state(now, ended);
            if state != GrantState::Active {
                return Err(format!(
                    "{id} is {}; only an active grant can be revoked",
                    state.as_str()
                ));
            }
        }
        let mut revoked = Vec::new();
        for grant in &mut self.write_grants {
            if which.is_none_or(|id| grant.id == id)
                && grant.state(now, ended) == GrantState::Active
            {
                grant.revoked_at = Some(now);
                grant.revoked_by = Some(by.to_owned());
                revoked.push(grant.id.clone());
            }
        }
        for id in &revoked {
            let key = format!("{id}:revoked");
            self.permission_audit.push(PermissionAudit {
                key: key.clone(),
                event: AuditEvent::AssistantWritePermissionRevoked,
                detail: json!({"key": key, "session": session, "grant": id, "by": by}),
            });
        }
        Ok(revoked)
    }
}

#[cfg(test)]
#[path = "write_permissions_tests.rs"]
mod tests;
