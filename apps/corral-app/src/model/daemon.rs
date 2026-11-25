//! Owned daemon snapshots, independent of the transport and the UI runtime. Rows hold the
//! `corral_types` values the daemon serves; the app derives groups and rails from them.
use crate::model::Id;
use corral_protocol::{Health, app::*, detail::*, rows::*};
use corral_types::{Event, Refusal, SystemRow, UsageSummary};
use std::collections::{BTreeMap, BTreeSet};

#[path = "daemon_board.rs"]
mod board;
pub use board::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Link {
    /// No session has delivered a complete board yet and none has failed.
    #[default]
    Connecting,
    /// A session is open and a complete board has been applied.
    Up,
    /// Health and board remain the last ones received.
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadStatus {
    Uploading,
    Ready,
    Failed(ClientError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadState {
    pub kind: corral_protocol::attachment::AttachmentKind,
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
    pub sent: u64,
    pub sha256: String,
    pub status: UploadStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TopicState {
    pub loading: bool,
    pub stale: bool,
    pub error: Option<ClientError>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetailCache {
    /// Presentation identities kept across transcript refreshes, never transcript content.
    pub transcript_seams: super::session::transcript::TranscriptSeams,
    pub target: DetailRef,
    pub pages: Vec<DetailReply>,
    pub loading: bool,
    pub stale: bool,
    pub error: Option<ClientError>,
    pub merged: Option<DetailBody>,
}

impl DetailCache {
    pub fn body(&self) -> Option<&DetailBody> {
        self.merged.as_ref()
    }
    pub fn bodies(&self) -> impl Iterator<Item = &DetailBody> {
        self.pages.iter().filter_map(|page| page.body.as_ref())
    }
}

pub fn detail_key(target: &DetailRef) -> String {
    // All selectors contain only strings/enums, so serialization cannot fail.
    serde_json::to_string(target).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus {
    Queued,
    Pending,
    Running,
    Done,
    Failed,
    Refused,
    Conflict,
    Unknown,
    Abandoned,
}

/// A message this device sent through its outbox (`session.send`), kept until its outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandState {
    pub device: Option<String>,
    pub method: String,
    pub request: serde_json::Value,
    pub status: CommandStatus,
    pub error: Option<ClientError>,
}

/// One request answered on the connection: its reply value, or its error. An `op.submit` the
/// engine refused carries the typed refusal with its next steps.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub method: String,
    pub loading: bool,
    pub value: Option<serde_json::Value>,
    pub error: Option<ClientError>,
    pub refusal: Option<Refusal>,
}

impl Operation {
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_value(self.value.as_ref()?.clone()).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Feed {
    pub link: Link,
    /// A one-line connection problem; absent while up.
    pub problem: Option<String>,
    pub health: Option<Health>,
    pub board: Board,
    pub session: Option<AppSession>,
    /// Current transport welcome, authenticated identity and projection readiness are distinct.
    pub transport_ready: bool,
    pub projection_ready: bool,
    pub capabilities: BTreeSet<String>,
    pub date: String,
    /// Driver-supplied local timezone; absent when the platform cannot determine it.
    pub time_zone: Option<jiff::tz::TimeZone>,
    /// Parent tasks with their children's counts, by parent id.
    pub plans: BTreeMap<Id, PlanRow>,
    /// Configured workspace projects (`Topic::Projects`, `projects.v1`). With the topic's
    /// complete page applied, an empty map means the workspace has no project management.
    pub projects: BTreeMap<Id, ProjectRow>,
    /// Each project's fact observation (`Topic::System`, `system.v1`), by project id: whether
    /// its map is building, the configured extension and its last error.
    pub systems: BTreeMap<Id, SystemRow>,
    /// Live runs by `<task>_<run>`, the same id as their terminal scope.
    pub agents: BTreeMap<Id, AgentRow>,
    pub assistants: BTreeMap<Id, AssistantRow>,
    /// The engine's usage totals and their run coverage.
    pub usage: Option<UsageSummary>,
    /// Parsed `corral.toml`, the listener and the devices.
    pub settings: Option<SettingsSnapshot>,
    pub topics: BTreeMap<String, TopicState>,
    pub details: BTreeMap<String, DetailCache>,
    /// Transcript item continuations, by continuation token.
    pub items: BTreeMap<String, crate::model::session::transcript::ItemFetch>,
    /// What each watched session streams (its open turn, fleet, host state and input
    /// admissions), by session scope.
    pub live: BTreeMap<Id, crate::model::session::transcript::LiveSession>,
    pub commands: BTreeMap<Id, CommandState>,
    pub operations: BTreeMap<Id, Operation>,
    /// Upload metadata only; file bytes never enter the model or persistent storage.
    pub uploads: BTreeMap<Id, UploadState>,
    pub changes: BTreeMap<u64, Event>,
    pub changes_head: u64,
    /// Canonical owner-relevant count, tied to the seen cursor used by the reader.
    pub changes_unseen: Option<(u64, u64)>,
    pub changes_older: Option<u64>,
    pub seen: Option<SeenCursor>,
    pub drafts: BTreeMap<Id, String>,
    pub device: crate::model::devices::DeviceLink,
    pub devices: Vec<crate::model::devices::DeviceRow>,
    /// Whether `devices` is the current registry, rather than retained or seeded rows.
    pub registry: crate::model::devices::Registry,
    pub pairing: crate::model::devices::Pairing,
    pub browser_pairing: crate::model::devices::BrowserPairing,
    /// Pairing-link draft, held only in memory and never written to app preferences.
    pub pairing_code: Option<String>,
    pub now: u64,
    pub push: Option<corral_protocol::device::PushStatus>,
    /// Local wall clock of `now`, HH:MM.
    pub clock: String,
    /// Local wall clock of the last daemon message.
    pub synced: Option<String>,
    /// Store directory path reported by the daemon, e.g. `/workspace/.corral`.
    pub store: Option<String>,
    /// Parent directory of `store`, when it has one.
    pub workspace: Option<String>,
    /// This device's hostname, captured once when the client starts.
    pub host: Option<String>,
    pub endpoint: Option<String>,
    /// The sample preview's pi hosts by session id (`model::preview`); corrald's hosts run
    /// behind corrald, so its feed never holds any.
    pub hosts: BTreeMap<Id, crate::model::preview::Host>,
    /// The sample preview's `corral.toml` text, which its `config.get` and `config.put` read and
    /// replace; corrald's file stays on the daemon host, so its feed never holds one.
    pub sample_config: Option<String>,
    /// Hosted tool overrides for the explicit fixture, never populated by daemon replies.
    pub sample_pi_hosted_tools: Option<corral_protocol::config::PiHostedToolsConfig>,
}

impl Feed {
    pub fn seen_received(&mut self, seen: SeenCursor) {
        if self.seen.as_ref().map(|cursor| cursor.through) != Some(seen.through) {
            self.changes_unseen = None;
        }
        self.seen = Some(seen);
    }
    pub fn capable(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn detail(&self, target: &DetailRef) -> Option<&DetailCache> {
        self.details.get(&detail_key(target))
    }

    pub fn settle_session(&mut self, target: &DetailRef, reread: Option<u64>) {
        if let DetailRef::Session { id } = target
            && let Some(reread) = reread
            && let Some(live) = self.live.get_mut(id.as_str())
        {
            live.settle(reread);
        }
    }

    pub fn ready(&self) -> bool {
        self.transport_ready
            && self.session.is_some()
            && self.projection_ready
            && self.link == Link::Up
    }

    pub fn mark_stale(&mut self) {
        self.transport_ready = false;
        self.projection_ready = false;
        self.session = None;
        self.registry = crate::model::devices::Registry::Unknown;
        for state in self.topics.values_mut() {
            state.stale = true;
            state.loading = false;
        }
        for detail in self.details.values_mut() {
            detail.stale = true;
            detail.loading = false;
        }
        for session in self.live.values_mut() {
            session.reset();
        }
        for command in self.commands.values_mut() {
            if matches!(
                command.status,
                CommandStatus::Pending | CommandStatus::Running
            ) {
                command.status = CommandStatus::Unknown;
            }
        }
        for operation in self.operations.values_mut() {
            if operation.loading {
                operation.loading = false;
                // A message may have reached the host before the link dropped; the host takes
                // an input id once, so it is never sent again on its own.
                let message = if operation.method == corral_protocol::session_host::SESSION_SEND {
                    SEND_LOST
                } else {
                    "Connection lost before a reply was received."
                };
                operation.error = Some(ClientError {
                    code: "disconnected".into(),
                    message: message.into(),
                });
            }
        }
    }
    /// An explicit link (including a refused one) must never resume an old browser identity.
    pub fn browser_pairing_pending(&self) -> bool {
        self.pairing_code.is_some()
            || matches!(
                self.browser_pairing,
                crate::model::devices::BrowserPairing::Blocked(_)
            )
    }

    pub fn connecting(&mut self, endpoint: String) {
        self.endpoint = Some(endpoint);
        self.link = Link::Connecting;
        self.problem
            .get_or_insert_with(|| "Waiting for the daemon board".into());
    }

    pub fn welcome_received(&mut self, store: String, health: Health) {
        self.transport_ready = true;
        self.workspace = std::path::Path::new(&store)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .and_then(|parent| parent.to_str())
            .map(str::to_owned);
        self.store = Some(store);
        self.health_received(health);
    }

    pub fn health_received(&mut self, health: Health) {
        self.health = Some(health);
        if self.ready() {
            self.synced = Some(self.clock.clone());
        }
    }

    pub fn begin_epoch(&mut self, epoch: u64) {
        self.board.begin_epoch(epoch);
    }

    pub fn board_received(
        &mut self,
        epoch: u64,
        rows: Vec<Task>,
        left: &[String],
        replacement: bool,
    ) {
        if replacement {
            self.begin_epoch(epoch);
        }
        self.board.apply_batch(epoch, rows, left);
        self.projection_ready = true;
        if self.transport_ready && self.session.is_some() {
            self.link = Link::Up;
            self.problem = None;
        }
        self.synced = Some(self.clock.clone());
    }

    pub fn link_lost(&mut self, problem: String) {
        self.link = Link::Down;
        self.mark_stale();
        self.problem = Some(if problem.contains(['\r', '\n']) {
            problem.replace(['\r', '\n'], " ")
        } else {
            problem
        });
    }

    pub fn revoked(&mut self, id: String) {
        self.device = crate::model::devices::DeviceLink::Revoked { id };
        self.board = Board::default();
        self.session = None;
        self.plans.clear();
        self.agents.clear();
        self.assistants.clear();
        self.usage = None;
        self.settings = None;
        self.details.clear();
        // A watched session's turn and child agents are daemon facts this device may no
        // longer read.
        self.live.clear();
        self.uploads.clear();
        self.changes.clear();
        self.changes_unseen = None;
        self.capabilities.clear();
        self.link_lost("This device was revoked.".into());
    }

    /// A completed device-page walk on this connection: the whole current registry.
    pub fn devices_received(&mut self, rows: Vec<crate::model::devices::DeviceRow>) {
        self.devices = rows;
        self.registry = crate::model::devices::Registry::Current;
    }

    pub fn tick(&mut self, now: u64, clock: String) {
        self.now = now;
        self.clock = clock;
    }

    /// `quick` names the newest quick chat that has not ended. Once the chat its page was read
    /// for has ended, that transcript and its live turn are the chat's own (`A-n`), not
    /// `quick`'s: they give way to the empty page of a new thread.
    pub fn forget_ended_quick(&mut self) {
        let quick = corral_protocol::assistant::QUICK_THREAD;
        let target = DetailRef::Session { id: quick.into() };
        let key = detail_key(&target);
        let ended = self
            .details
            .get(&key)
            .and_then(|cache| match cache.body() {
                Some(DetailBody::Session(page)) if page.id != quick => Some(page.id.as_str()),
                _ => None,
            })
            .and_then(|chat| self.assistants.get(chat))
            .is_some_and(|row| row.state == AssistantState::Ended);
        if !ended {
            return;
        }
        self.details.insert(
            key,
            DetailCache {
                transcript_seams: Default::default(),
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Session(empty_quick_page())),
            },
        );
        if let Some(live) = self.live.get_mut(quick) {
            live.clear_host();
        }
    }
}

/// The page of `quick` while no quick chat is current: no messages, taking the message that
/// starts one, its host not started.
pub fn empty_quick_page() -> SessionPage {
    SessionPage {
        id: corral_protocol::assistant::QUICK_THREAD.into(),
        role: "assistant".into(),
        task: None,
        run: None,
        machine: None,
        messages: Vec::new(),
        older: None,
        replyable: true,
        reply_refusal: None,
        host: HostPresence::NotRunning,
    }
}

/// Why a message whose connection dropped before corrald answered was not sent again.
pub const SEND_LOST: &str = "the connection dropped before corrald answered; it may have arrived, so check the conversation before sending it again";

/// The shared task-card age formatter.
pub fn age(since: Option<u64>, now: u64) -> String {
    crate::model::work::age(since, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_types::Status;

    fn task(id: &str) -> Task {
        Task::new(task_row(bare_task(
            id,
            id,
            Status::Ready,
            jiff::Timestamp::UNIX_EPOCH,
        )))
    }

    #[test]
    fn epoch_replacement_keeps_old_rows_until_snapshot_completes() {
        let mut feed = Feed::default();
        feed.board_received(1, vec![task("T-9")], &[], true);
        feed.begin_epoch(2);
        assert_eq!(feed.board.epoch, 1);
        assert!(feed.board.task("T-9").is_some());
        feed.board_received(2, vec![task("T-10")], &[], false);
        assert_eq!(feed.board.epoch, 2);
        assert!(feed.board.task("T-9").is_none());
        assert!(feed.board.task("T-10").is_some());
        assert!(!feed.ready());
    }

    /// Ending the quick chat leaves its page cached under `quick`; Ask, back on `quick`, must
    /// then show a new, empty thread rather than the ended thread's transcript.
    #[test]
    fn an_ended_threads_transcript_is_not_shown_as_the_new_thread() {
        use crate::model::effect::{Conn, Env};
        use crate::model::session::{self, Body};
        let mut feed = Feed::default();
        let target = DetailRef::Session { id: "quick".into() };
        let mut page = empty_quick_page();
        page.id = "A-1".into();
        page.host = HostPresence::Running;
        page.messages.push(SessionMessage {
            id: "A-1/entry/1".into(),
            author: "user".into(),
            at: Some("2026-11-19T10:00:00Z".into()),
            items: vec![SessionItem::Text {
                text: "the ended thread's question".into(),
            }],
            model: None,
            stop_reason: None,
            usage: None,
            input: None,
            attachments: Vec::new(),
        });
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Session(page)),
            },
        );
        let ended: AssistantRow = serde_json::from_value(serde_json::json!({
            "id": "A-1", "kind": "quick", "title": "Quick thread", "state": "ended",
            "live_in": "none", "profile": null, "connection": null, "focus": null, "pending": 0,
            "turn": 1, "cost_usd": null, "cost_basis": null, "wait": null, "rev": 2,
        }))
        .expect("an ended quick chat row");
        feed.assistants.insert(Id::from("A-1"), ended);
        feed.forget_ended_quick();
        let env = Env {
            conn: Conn::Healthy,
            form: crate::model::Form::Desktop,
        };
        let shown = session::page(&feed, &session::State::default(), &Id::from("quick"), env);
        let Body::Chat(chat) = shown.body else {
            panic!("Ask on quick shows its chat");
        };
        assert!(chat.transcript.messages.is_empty());
        assert!(
            chat.empty.is_some(),
            "a new thread says what the first message does"
        );
    }
}
