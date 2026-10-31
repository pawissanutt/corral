//! Workspace-scoped assistant records. No task, run, queue entry or coding slot is allocated.
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agent::{
    AgentBinding, AgentRole, AgentRunState, GenerationFence, HostHealth, WakeState,
};
use crate::{Store, Usage};

mod records;

pub const ASSISTANT_VERSION: u32 = 1;
/// Owner inputs a container keeps; older ones leave the journal and can no longer be cited.
pub const OWNER_INPUTS_MAX: usize = 64;
/// Bytes of one journaled owner input's text; a longer input keeps its full digest.
pub const OWNER_INPUT_TEXT_MAX: usize = 16 * 1024;

/// `[assistant]`: launch defaults. `max_live` is soft: `corral pi --over-cap` opens one more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantCfg {
    /// The pi-native profile assistants run; unset means the PM's profile (`pm.agent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Live assistant hosts a plain `corral pi` may reach before it needs `--over-cap`.
    pub max_live: u32,
    /// Scratch worktrees the workspace may hold across all sessions.
    pub scratch_max: u32,
    /// Days an ended session's scratch stays idle before `corral assistant scratch gc` collects it.
    pub scratch_idle_days: u32,
}

impl Default for AssistantCfg {
    fn default() -> Self {
        Self {
            agent: None,
            max_live: 4,
            scratch_max: 8,
            scratch_idle_days: 7,
        }
    }
}

impl AssistantCfg {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

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
    pub artifacts: Vec<crate::Artifact>,
    #[serde(default)]
    pub inbox_items: Vec<String>,
    #[serde(default)]
    pub landed: Vec<AssistantLanding>,
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

/// A co-work task this session finished that landed: its run, the base it landed on, the
/// landed head and the commits the landing added to that base, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantLanding {
    pub task: String,
    pub run: String,
    pub base: String,
    pub sha: String,
    pub commits: Vec<String>,
    pub at: Timestamp,
}

/// The pane is adopted, never owned: ending a session must not close or type into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantContainer {
    pub version: u32,
    pub pane: Option<String>,
    pub herdr_socket: Option<PathBuf>,
    pub workspace: PathBuf,
    pub state: AgentRunState,
    /// The owner-input journal, oldest first: the only inputs an owner action may cite.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owner_inputs: Vec<OwnerInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerInputSource {
    /// Typed into the session's pane by the owner (Pi input source `interactive`).
    Pane,
    /// The owner's answer to an owner-action confirmation the host rendered.
    Confirmation,
}

/// OwnerInput journal entry v1. Only the assistant host's `event` op writes one, before the
/// text reaches the model; corral wakes, inbox deliveries, PM outcomes and recovery notices
/// never become one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerInput {
    /// `A-<n>/g<generation>/i<seq>`.
    pub id: String,
    pub generation: u32,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    pub source: OwnerInputSource,
    /// SHA-256 of the complete input; `text` keeps at most `OWNER_INPUT_TEXT_MAX` bytes of it.
    pub sha256: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Confirmations only: the exact command line the host showed the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Confirmations only: the owner-action request (tool request ID) the answer answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<String>,
    /// The host's `event` request that journaled it.
    pub request: String,
    pub at: Timestamp,
}

/// What a host journals; the container assigns its ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOwnerInput {
    pub generation: u32,
    pub turn: Option<u64>,
    pub source: OwnerInputSource,
    pub sha256: String,
    pub text: String,
    pub truncated: bool,
    pub command: Option<String>,
    pub answers: Option<String>,
    pub request: String,
}

impl AssistantContainer {
    pub fn owner_input(&self, id: &str) -> Option<&OwnerInput> {
        self.owner_inputs.iter().find(|input| input.id == id)
    }

    /// The owner's latest journaled answer to a confirmation of `command` for `request`.
    pub fn confirmation(&self, request: &str, command: &str) -> Option<&OwnerInput> {
        self.owner_inputs.iter().rev().find(|input| {
            input.source == OwnerInputSource::Confirmation
                && input.answers.as_deref() == Some(request)
                && input.command.as_deref() == Some(command)
        })
    }

    /// Journals `input` as session `session`'s next `i<seq>` of its generation, dropping the
    /// oldest entries beyond `OWNER_INPUTS_MAX`. The newest entry is never the one dropped, so
    /// sequence numbers never repeat within a generation.
    pub fn journal_owner_input(&mut self, session: &str, input: NewOwnerInput) -> OwnerInput {
        let seq = self
            .owner_inputs
            .iter()
            .filter(|entry| entry.generation == input.generation)
            .map(|entry| entry.seq)
            .max()
            .unwrap_or(0)
            + 1;
        let entry = OwnerInput {
            id: format!("{session}/g{}/i{seq}", input.generation),
            generation: input.generation,
            seq,
            turn: input.turn,
            source: input.source,
            sha256: input.sha256,
            text: input.text,
            truncated: input.truncated,
            command: input.command,
            answers: input.answers,
            request: input.request,
            at: Timestamp::now(),
        };
        self.owner_inputs.push(entry.clone());
        let excess = self.owner_inputs.len().saturating_sub(OWNER_INPUTS_MAX);
        self.owner_inputs.drain(..excess);
        entry
    }
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
    /// Read-only artifact-owner projection for shared pin/search/retention readers.
    /// These records are never task files and never enter scheduling or the board.
    pub fn artifact_owners(&self) -> Result<Vec<crate::Task>> {
        let mut owners = self.list()?;
        owners.extend(self.assistant_artifact_owners()?);
        Ok(owners)
    }

    /// Session contexts only, for readers which already have the task listing.
    pub fn assistant_artifact_owners(&self) -> Result<Vec<crate::Task>> {
        let mut owners = Vec::new();
        // Atomic record replacement makes this safe without taking the board lock.
        // Indexing also runs inside dispatch, so recursively acquiring it would deadlock.
        for session in self.read_assistants()? {
            owners.push(
                serde_json::from_value(serde_json::json!({
                    "id": session.id, "title": session.summary,
                    "status": if session.ended.is_some() { "done" } else { "backlog" },
                    "created": session.created, "updated": session.ended.unwrap_or(session.created),
                    "artifacts": session.artifacts,
                }))
                .context("building assistant artifact context")?,
            );
        }
        Ok(owners)
    }

    /// Read-only scratch rendering of the canonical session record.
    pub fn assistant_path(&self, id: &str) -> Result<PathBuf> {
        records::materialize_session(self, id)
    }

    /// Read-only scratch rendering; host state is authoritative only in records.
    pub fn assistant_container_path(&self, id: &str) -> Result<PathBuf> {
        records::materialize_container(self, id)
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
        let next = records::reserve(self)?;
        let session = AssistantSession {
            version: ASSISTANT_VERSION,
            id: format!("A-{next}"),
            created: self.now(),
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
            owner_inputs: Vec::new(),
        };
        self.records_transaction(|| {
            self.write_assistant_container(&session.id, &container)?;
            self.write_assistant(&session)?;
            self.log_event(
                None,
                "assistant_created",
                serde_json::json!({"session": session.id}),
            )
        })?;
        Ok(session)
    }

    pub(crate) fn read_assistant(&self, id: &str) -> Result<AssistantSession> {
        records::session(self, id)
    }

    pub fn assistant_exists(&self, id: &str) -> Result<bool> {
        validate_id(id)?;
        Ok(self.record_row("assistant-sessions", id)?.is_some())
    }

    pub fn get_assistant(&self, id: &str) -> Result<AssistantSession> {
        self.read_assistant(id)
    }

    pub fn list_assistants(&self) -> Result<Vec<AssistantSession>> {
        self.read_assistants()
    }

    fn read_assistants(&self) -> Result<Vec<AssistantSession>> {
        records::sessions(self)
    }

    fn read_assistant_container(&self, id: &str) -> Result<AssistantContainer> {
        records::container(self, id)
    }

    pub fn get_assistant_container(&self, id: &str) -> Result<AssistantContainer> {
        self.read_assistant_container(id)
    }

    fn write_assistant(&self, session: &AssistantSession) -> Result<()> {
        records::save_session(self, session)
    }

    fn write_assistant_container(&self, id: &str, container: &AssistantContainer) -> Result<()> {
        records::save_container(self, id, container)
    }

    /// Serialize host updates with lifecycle operations. Never call from inside a task update.
    pub fn update_assistant_container<R>(
        &self,
        id: &str,
        f: impl FnOnce(&mut AssistantContainer) -> Result<R>,
    ) -> Result<R> {
        self.change_assistant_container(id, false, |container, _| f(container))
    }

    /// `update_assistant_container` that also reaches an ended session, telling `f` whether it
    /// ended: post-commit bookkeeping and an ended session's fenced receipts stay settleable.
    pub fn settle_assistant_container<R>(
        &self,
        id: &str,
        f: impl FnOnce(&mut AssistantContainer, bool) -> Result<R>,
    ) -> Result<R> {
        self.change_assistant_container(id, true, f)
    }

    fn change_assistant_container<R>(
        &self,
        id: &str,
        allow_ended: bool,
        f: impl FnOnce(&mut AssistantContainer, bool) -> Result<R>,
    ) -> Result<R> {
        let _board = self.lock("dispatch")?;
        let ended = self.read_assistant(id)?.ended.is_some();
        if ended && !allow_ended {
            bail!("assistant_ended: {id}");
        }
        let mut container = self.read_assistant_container(id)?;
        let result = f(&mut container, ended)?;
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
        let now = self.now();
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
        session.ended = Some(now);
        self.records_transaction(|| {
            self.write_assistant_container(id, &container)?;
            self.write_assistant(&session)?;
            self.log_event(None, "assistant_ended", serde_json::json!({"session": id}))
        })?;
        Ok(session)
    }
}
