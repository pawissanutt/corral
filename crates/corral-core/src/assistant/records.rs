//! Canonical assistant records and their derived live-session keys. Paths are read-only
//! scratch materializations; neither markdown nor JSON files are a storage authority.
use super::{ASSISTANT_VERSION, AssistantContainer, AssistantSession, validate_id};
use crate::Store;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const SESSIONS: &str = "assistant-sessions";
const CONTAINERS: &str = "assistant-containers";
const LIVE: &str = "assistant-live";

#[derive(Serialize, Deserialize)]
struct SessionRecord {
    session: AssistantSession,
    body: String,
}

pub(super) fn session(store: &Store, id: &str) -> Result<AssistantSession> {
    validate_id(id)?;
    let record: SessionRecord = store
        .record(SESSIONS, id)?
        .with_context(|| format!("assistant {id} not found"))?;
    let mut session = record.session;
    if session.version != ASSISTANT_VERSION || session.id != id {
        bail!("unsupported or mismatched assistant record {id}");
    }
    session.body = record.body;
    Ok(session)
}

pub(super) fn sessions(store: &Store) -> Result<Vec<AssistantSession>> {
    let mut sessions = Vec::new();
    for row in store.record_rows(SESSIONS)? {
        validate_id(&row.key)?;
        let record: SessionRecord =
            serde_json::from_value(row.doc).context("decoding assistant record")?;
        if record.session.version != ASSISTANT_VERSION || record.session.id != row.key {
            bail!("unsupported or mismatched assistant record {}", row.key);
        }
        let mut session = record.session;
        session.body = record.body;
        sessions.push(session);
    }
    sessions.sort_by_key(|session| (session.created, session.id.clone()));
    Ok(sessions)
}

pub(super) fn container(store: &Store, id: &str) -> Result<AssistantContainer> {
    validate_id(id)?;
    let value: AssistantContainer = store
        .record(CONTAINERS, id)?
        .with_context(|| format!("assistant container {id} not found"))?;
    if value.version != ASSISTANT_VERSION
        || value.state.binding.role != crate::agent::AgentRole::Assistant
    {
        bail!("unsupported assistant container {id}");
    }
    Ok(value)
}

pub(super) fn save_session(store: &Store, session: &AssistantSession) -> Result<()> {
    validate_id(&session.id)?;
    store.records_transaction(|| {
        store.save_record(
            SESSIONS,
            &session.id,
            &SessionRecord {
                session: session.clone(),
                body: session.body.clone(),
            },
        )?;
        if session.ended.is_none() {
            store.save_record(LIVE, &session.id, &true)
        } else {
            store.delete_record(LIVE, &session.id)
        }
    })
}

pub(super) fn save_container(store: &Store, id: &str, value: &AssistantContainer) -> Result<()> {
    validate_id(id)?;
    store.save_record(CONTAINERS, id, value)
}

/// Dispatch serializes the counter. Reserve it before the publication unit, retaining real gaps.
pub(super) fn reserve(store: &Store) -> Result<u64> {
    let previous = store
        .record::<u64>("assistant-counter", "current")?
        .unwrap_or(0);
    let mut next = previous
        .checked_add(1)
        .context("assistant counter overflow")?;
    loop {
        let id = format!("A-{next}");
        if store.record::<serde_json::Value>(SESSIONS, &id)?.is_none()
            && store
                .record::<serde_json::Value>(CONTAINERS, &id)?
                .is_none()
        {
            break;
        }
        next = next.checked_add(1).context("assistant counter overflow")?;
    }
    store.save_record("assistant-counter", "current", &next)?;
    Ok(next)
}

pub(super) fn materialize_session(store: &Store, id: &str) -> Result<std::path::PathBuf> {
    let session = session(store, id)?;
    let dir = store.scratch_spool(
        crate::backend::ScratchKind::TmpStaging,
        "assistant-readback",
    )?;
    std::fs::create_dir_all(&dir).context("creating assistant readback directory")?;
    let path = dir.join(format!("{id}.md"));
    let yaml = serde_yaml_ng::to_string(&session).context("rendering assistant metadata")?;
    crate::store::atomic_write(&path, &format!("---\n{yaml}---\n{}", session.body))?;
    read_only(&path)?;
    Ok(path)
}

pub(super) fn materialize_container(store: &Store, id: &str) -> Result<std::path::PathBuf> {
    let container = container(store, id)?;
    let dir = store.scratch_spool(
        crate::backend::ScratchKind::TmpStaging,
        "assistant-readback",
    )?;
    std::fs::create_dir_all(&dir).context("creating assistant readback directory")?;
    let path = dir.join(format!("{id}.json"));
    crate::store::atomic_write(
        &path,
        &serde_json::to_string_pretty(&container).context("rendering assistant container")?,
    )?;
    read_only(&path)?;
    Ok(path)
}

fn read_only(path: &std::path::Path) -> Result<()> {
    let mut permissions = std::fs::metadata(path)
        .context("reading assistant readback permissions")?
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions).context("protecting assistant readback")
}

impl Store {
    /// IDs of live sessions only, from derived keys published with each canonical session write.
    pub fn active_assistant_ids(&self) -> Result<Vec<String>> {
        self.record_rows(LIVE)?
            .into_iter()
            .map(|row| {
                validate_id(&row.key)?;
                Ok(row.key)
            })
            .collect()
    }
}
