//! `corral assistant permissions` (T-656 grant-store): a session's write grants with their
//! states, the running generation's and the saved write_permissions settings, revocation, and
//! the drain of the session's durable audit outbox.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_core::Store;
use corral_core::agent::write_permissions::{
    GrantState, Home, WriteGrant, WritePermissionSettings, resolve_allow,
};
use corral_core::agent::{AgentRole, CapabilityState};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::Value;

const FEATURE: &str = "write_permissions";

/// Resolve a directory without creating it: canonicalize its existing ancestor, then append
/// missing components, just as the host does before Pi creates a new file's parent directories.
/// A dangling symlink, inaccessible ancestor or existing non-directory never resolves.
pub fn resolve_dir(path: &Path) -> Option<PathBuf> {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => {
                let mut canonical = std::fs::canonicalize(existing).ok()?;
                if !canonical.is_dir() {
                    return None;
                }
                canonical.extend(missing.into_iter().rev());
                return Some(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(existing.file_name()?);
                existing = existing.parent()?;
            }
            Err(_) => return None,
        }
    }
}

/// Writes every queued audit line of `session` with its key, then clears the written keys.
/// A crash between the two leaves the entries queued, and the next drain repeats their lines
/// with the same keys. Returns how many lines it wrote.
pub fn drain_permission_audit(store: &Store, session: &str) -> Result<usize> {
    let queued = store.inspect_assistant_container(session, |container, _| {
        Ok(container.permission_audit.clone())
    })?;
    if queued.is_empty() {
        return Ok(0);
    }
    for entry in &queued {
        store
            .log_event(None, entry.event.as_str(), entry.detail.clone())
            .with_context(|| format!("logging {} {}", entry.event.as_str(), entry.key))?;
    }
    crate::agent::failpoint("permission_audit_logged_before_clear");
    store.settle_assistant_container(session, |container, _| {
        container
            .permission_audit
            .retain(|entry| !queued.iter().any(|written| written.key == entry.key));
        Ok(())
    })?;
    Ok(queued.len())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Revoked {
    pub session: String,
    pub revoked: Vec<String>,
    pub by: String,
    /// The revocation applied, but its audit line is still queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Revokes `grant` (or, with `None`, every active grant) of `session` for `by`: `owner` or the
/// session itself. Records `revoked_at` and `revoked_by` and queues the audit line in one
/// container change, then drains; a failed drain is a warning, never an unrevoked grant.
pub fn revoke(store: &Store, session: &str, grant: Option<&str>, by: &str) -> Result<Revoked> {
    corral_core::assistant::validate_id(session)?;
    if by != "owner" && by != session {
        bail!("only the owner or {session}'s own shell can revoke its write grants");
    }
    if let Some(id) = grant {
        let n = id.strip_prefix("G-").unwrap_or_default();
        if n.is_empty() || n.starts_with('0') || !n.bytes().all(|b| b.is_ascii_digit()) {
            bail!("a write grant ID is G-<n> with n > 0");
        }
    }
    let now = Timestamp::now();
    let revoked = store.settle_assistant_container(session, |container, ended| {
        container
            .revoke_write_grants(session, grant, by, now, ended)
            .map_err(anyhow::Error::msg)
    })?;
    let warning = drain_permission_audit(store, session).err().map(|error| {
        format!("warning: the revocation applies, but its audit line stays queued ({error:#})")
    });
    Ok(Revoked {
        session: session.into(),
        revoked,
        by: by.into(),
        warning,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantRow {
    pub state: GrantState,
    #[serde(flatten)]
    pub grant: WriteGrant,
}

/// One allow entry and its current resolution, or why it is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllowRow {
    pub entry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsRow {
    pub enabled: bool,
    pub allow: Vec<AllowRow>,
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub session: String,
    pub ended: bool,
    pub generation: u32,
    pub grants: Vec<GrantRow>,
    /// The running generation's settings; `None` when its snapshot has no write_permissions key.
    pub generation_settings: Option<SettingsRow>,
    /// What a new generation would get from the saved configuration.
    pub saved_settings: Option<SettingsRow>,
    /// The saved configuration differs from the running generation's; it applies on resume.
    pub saved_differs: bool,
    /// Audit lines committed but not yet in the event log.
    pub audit_queued: usize,
}

fn settings_row(state: Option<&CapabilityState>, home: &Home) -> Result<Option<SettingsRow>> {
    let Some(state) = state else {
        return Ok(None);
    };
    let settings: WritePermissionSettings =
        serde_json::from_value(Value::Object(state.settings.clone().into_iter().collect()))
            .context("parsing recorded write_permissions settings")?;
    let allow = settings
        .allow
        .into_iter()
        .map(|entry| match resolve_allow(&entry, home, resolve_dir) {
            Ok(canonical) => AllowRow {
                entry,
                canonical: Some(canonical),
                ignored: None,
            },
            Err(why) => AllowRow {
                entry,
                canonical: None,
                ignored: Some(why),
            },
        })
        .collect();
    Ok(Some(SettingsRow {
        enabled: state.enabled,
        allow,
        deny: settings.deny,
    }))
}

/// Every grant record of `session` with its state at `now`, beside the generation's and the
/// saved settings with each allow entry resolved against `home` now.
pub fn list(store: &Store, session: &str, now: Timestamp, home: &Home) -> Result<Listing> {
    corral_core::assistant::validate_id(session)?;
    let (container, ended) = store
        .inspect_assistant_container(session, |container, ended| Ok((container.clone(), ended)))?;
    let running = container
        .state
        .current_generation()
        .map(|generation| generation.effective_capabilities(AgentRole::Assistant));
    let saved = store
        .config
        .agent
        .capabilities
        .resolve(AgentRole::Assistant, None);
    let running_state = running.as_ref().and_then(|s| s.features.get(FEATURE));
    let saved_state = saved.features.get(FEATURE);
    let fields = |state: Option<&CapabilityState>| state.map(|s| (s.enabled, s.settings.clone()));
    Ok(Listing {
        session: session.into(),
        ended,
        generation: container.state.binding.generation,
        grants: container
            .write_grants
            .iter()
            .map(|grant| GrantRow {
                state: grant.state(now, ended),
                grant: grant.clone(),
            })
            .collect(),
        generation_settings: settings_row(running_state, home)?,
        saved_settings: settings_row(saved_state, home)?,
        saved_differs: fields(running_state) != fields(saved_state),
        audit_queued: container.permission_audit.len(),
    })
}
