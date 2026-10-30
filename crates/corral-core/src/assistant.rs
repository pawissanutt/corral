//! Workspace-scoped assistant records. No task, run, queue entry or coding slot is allocated.
use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agent::{
    AgentBinding, AgentRole, AgentRunState, GenerationFence, HostHealth, WakeState,
};
use crate::{Store, Usage, store::atomic_write};

pub const ASSISTANT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantSession {
    pub version: u32,
    pub id: String,
    pub created: Timestamp,
    pub ended: Option<Timestamp>,
    /// `workspace` or a task ID. Focus does not grant task authority.
    pub focus: String,
    pub profile: String,
    #[serde(default)]
    pub scratch: Vec<AssistantScratch>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub inbox_items: Vec<String>,
    #[serde(default)]
    pub landed: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(skip)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantScratch {
    pub slug: String,
    pub project: String,
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub created: Timestamp,
    pub kind: String,
    pub bytes: Option<u64>,
    pub measured_at: Option<Timestamp>,
}

/// The pane is adopted, never owned: ending a session must not close or type into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantContainer {
    pub version: u32,
    pub pane: Option<String>,
    pub herdr_socket: Option<PathBuf>,
    pub workspace: PathBuf,
    pub state: AgentRunState,
}

pub fn validate_id(id: &str) -> Result<()> {
    let n = id
        .strip_prefix("A-")
        .context("assistant ID must be A-<n>")?;
    if n.parse::<u64>().ok().filter(|n| *n > 0).is_none()
        || n.starts_with('0')
        || !n.bytes().all(|b| b.is_ascii_digit())
    {
        bail!("assistant ID must be A-<n> with n > 0");
    }
    Ok(())
}

impl Store {
    pub fn assistant_path(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.dir().join("assistants").join(format!("{id}.md")))
    }

    pub fn assistant_container_path(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self
            .state_dir()
            .join("agent-assistant")
            .join(format!("{id}.json")))
    }

    /// Reserve the counter durably before writing either file; interrupted creates never reuse IDs.
    /// Both files become visible to store readers under the board writer lock.
    pub fn create_assistant(
        &self,
        profile: &str,
        focus: Option<&str>,
        pane: Option<&str>,
        herdr_socket: Option<PathBuf>,
    ) -> Result<AssistantSession> {
        let _board = self.lock("dispatch")?;
        let counter = self.state_dir().join("assistant-counter");
        let previous = match fs::read_to_string(&counter) {
            Ok(text) => text
                .trim()
                .parse::<u64>()
                .context("parsing assistant counter")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e).context("reading assistant counter"),
        };
        // Also protect records restored without their counter.
        let mut next = previous
            .checked_add(1)
            .context("assistant counter overflow")?;
        while self.assistant_path(&format!("A-{next}"))?.exists()
            || self
                .assistant_container_path(&format!("A-{next}"))?
                .exists()
        {
            next = next.checked_add(1).context("assistant counter overflow")?;
        }
        fs::create_dir_all(self.state_dir()).context("creating assistant state directory")?;
        atomic_write(&counter, &next.to_string())?;
        let session = AssistantSession {
            version: ASSISTANT_VERSION,
            id: format!("A-{next}"),
            created: Timestamp::now(),
            ended: None,
            focus: focus.unwrap_or("workspace").into(),
            profile: profile.into(),
            scratch: Vec::new(),
            artifacts: Vec::new(),
            inbox_items: Vec::new(),
            landed: Vec::new(),
            summary: String::new(),
            body: String::new(),
            usage: None,
        };
        let container = AssistantContainer {
            version: ASSISTANT_VERSION,
            pane: pane.map(str::to_owned),
            herdr_socket,
            workspace: self.root().to_path_buf(),
            state: AgentRunState::new(AgentBinding {
                role: AgentRole::Assistant,
                store: crate::agent::paths::store_id(self),
                project: String::new(),
                generation: 0,
                health: HostHealth::Unbound,
                callback: None,
                closing: false,
            }),
        };
        self.write_assistant_container(&session.id, &container)?;
        self.write_assistant(&session)?;
        self.log_event(
            None,
            "assistant_created",
            serde_json::json!({"session": session.id}),
        )?;
        Ok(session)
    }

    fn read_assistant(&self, id: &str) -> Result<AssistantSession> {
        let path = self.assistant_path(id)?;
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let text = text
            .strip_prefix("---\n")
            .context("assistant frontmatter missing")?;
        let (yaml, body) = text
            .split_once("\n---\n")
            .context("assistant frontmatter not closed")?;
        let mut session: AssistantSession =
            serde_yaml_ng::from_str(yaml).context("parsing assistant record")?;
        if session.version != ASSISTANT_VERSION || session.id != id {
            bail!("unsupported or mismatched assistant record {id}");
        }
        session.body = body.to_owned();
        Ok(session)
    }

    pub fn get_assistant(&self, id: &str) -> Result<AssistantSession> {
        let _board = self.lock("dispatch")?;
        self.read_assistant(id)
    }

    pub fn list_assistants(&self) -> Result<Vec<AssistantSession>> {
        let _board = self.lock("dispatch")?;
        let dir = self.dir().join("assistants");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&dir).context("listing assistants")? {
            let path = entry.context("reading assistant entry")?.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("invalid assistant filename")?;
                sessions.push(self.read_assistant(id)?);
            }
        }
        sessions.sort_by_key(|s| (s.created, s.id.clone()));
        Ok(sessions)
    }

    fn read_assistant_container(&self, id: &str) -> Result<AssistantContainer> {
        let path = self.assistant_container_path(id)?;
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let container: AssistantContainer =
            serde_json::from_str(&text).context("parsing assistant container")?;
        if container.version != ASSISTANT_VERSION
            || container.state.binding.role != AgentRole::Assistant
        {
            bail!("unsupported assistant container {id}");
        }
        Ok(container)
    }

    pub fn get_assistant_container(&self, id: &str) -> Result<AssistantContainer> {
        let _board = self.lock("dispatch")?;
        self.read_assistant_container(id)
    }

    fn write_assistant(&self, session: &AssistantSession) -> Result<()> {
        let path = self.assistant_path(&session.id)?;
        fs::create_dir_all(self.dir().join("assistants"))
            .context("creating assistants directory")?;
        let yaml = serde_yaml_ng::to_string(session).context("serializing assistant")?;
        atomic_write(&path, &format!("---\n{yaml}---\n{}", session.body))
    }

    fn write_assistant_container(&self, id: &str, container: &AssistantContainer) -> Result<()> {
        let path = self.assistant_container_path(id)?;
        fs::create_dir_all(self.state_dir().join("agent-assistant"))
            .context("creating assistant containers")?;
        atomic_write(
            &path,
            &serde_json::to_string_pretty(container).context("serializing assistant container")?,
        )
    }

    /// Serialize host updates with lifecycle operations. Never call from inside a task update.
    pub fn update_assistant_container<R>(
        &self,
        id: &str,
        f: impl FnOnce(&mut AssistantContainer) -> Result<R>,
    ) -> Result<R> {
        let _board = self.lock("dispatch")?;
        let session = self.read_assistant(id)?;
        if session.ended.is_some() {
            bail!("assistant_ended: {id}");
        }
        let mut container = self.read_assistant_container(id)?;
        let result = f(&mut container)?;
        self.write_assistant_container(id, &container)?;
        Ok(result)
    }

    pub fn update_assistant(
        &self,
        id: &str,
        f: impl FnOnce(&mut AssistantSession) -> Result<()>,
    ) -> Result<AssistantSession> {
        let _board = self.lock("dispatch")?;
        let mut session = self.read_assistant(id)?;
        f(&mut session)?;
        if session.id != id || session.version != ASSISTANT_VERSION {
            bail!("assistant identity is immutable");
        }
        self.write_assistant(&session)?;
        Ok(session)
    }

    /// Idempotent closure: fence generations and cancel undelivered wakes, retaining transcripts.
    /// Write container first: if interrupted, repeating end completes the session record.
    pub fn end_assistant(&self, id: &str) -> Result<AssistantSession> {
        let _board = self.lock("dispatch")?;
        let mut session = self.read_assistant(id)?;
        if session.ended.is_some() {
            return Ok(session);
        }
        let now = Timestamp::now();
        let mut container = self.read_assistant_container(id)?;
        container.state.binding.closing = true;
        container.state.binding.health = HostHealth::Fenced;
        for generation in &mut container.state.generations {
            generation.fenced.get_or_insert(GenerationFence {
                reason: "assistant ended".into(),
                at: now,
            });
        }
        for wake in &mut container.state.wakes {
            if matches!(wake.state, WakeState::Armed | WakeState::PendingStart) {
                wake.state = WakeState::Cancelled;
            }
        }
        self.write_assistant_container(id, &container)?;
        session.ended = Some(now);
        self.write_assistant(&session)?;
        self.log_event(None, "assistant_ended", serde_json::json!({"session": id}))?;
        Ok(session)
    }
}
