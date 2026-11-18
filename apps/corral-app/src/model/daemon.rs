//! Owned daemon snapshots, independent of the transport and the UI runtime. Rows hold the
//! `corral_types` values the daemon serves; the app derives groups and rails from them.
use crate::model::Id;
use corral_protocol::{
    Health, app::*, assistant::ThreadEntry, detail::*, rows::*, subscription::LiveText,
};
use corral_types::{Event, Refusal, UsageSummary};
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

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ThreadSnapshot {
    pub entries: BTreeMap<Id, ThreadEntry>,
    pub live: Option<LiveText>,
    pub stale: bool,
    pub loading: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DetailCache {
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

/// A chat send or prompt answer this device made, kept until its echo or outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandState {
    pub device: Option<String>,
    pub method: String,
    pub request: serde_json::Value,
    pub status: CommandStatus,
    pub error: Option<ClientError>,
    /// The thread entry corrald acknowledged for an assistant send; kept with the command, never
    /// in the bounded reply cache, so the local copy stays merged into it.
    pub entry: Option<Id>,
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

/// Local ownership transfer, published only after the outbox write succeeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendAcceptance {
    Accepted,
    Refused(ClientError),
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
    /// Live runs by `<task>_<run>`, the same id as their terminal scope.
    pub agents: BTreeMap<Id, AgentRow>,
    pub assistants: BTreeMap<Id, AssistantRow>,
    /// The engine's usage totals and their run coverage.
    pub usage: Option<UsageSummary>,
    /// Parsed `corral.toml`, the listener and the devices.
    pub settings: Option<SettingsSnapshot>,
    pub threads: BTreeMap<Id, ThreadSnapshot>,
    pub topics: BTreeMap<String, TopicState>,
    pub details: BTreeMap<String, DetailCache>,
    /// Transcript item continuations, by continuation token.
    pub items: BTreeMap<String, crate::model::session_feed::ItemFetch>,
    /// The open turn of each watched session, by session id.
    pub live: BTreeMap<Id, crate::model::session_feed::LiveSession>,
    pub commands: BTreeMap<Id, CommandState>,
    pub send_acceptance: BTreeMap<Id, SendAcceptance>,
    pub operations: BTreeMap<Id, Operation>,
    /// Upload metadata only; file bytes never enter the model or persistent storage.
    pub uploads: BTreeMap<Id, UploadState>,
    pub cancelling: BTreeSet<Id>,
    pub not_withdrawn: BTreeSet<Id>,
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
        for thread in self.threads.values_mut() {
            thread.stale = true;
            thread.loading = false;
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
                operation.error = Some(ClientError {
                    code: "disconnected".into(),
                    message: "Connection lost before a reply was received.".into(),
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
        self.threads.clear();
        self.details.clear();
        // A watched session's turn and child agents are daemon facts this device may no
        // longer read.
        self.live.clear();
        self.uploads.clear();
        self.cancelling.clear();
        self.not_withdrawn.clear();
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

    /// Retires delivered sends whose acknowledged entry this device has seen in their thread:
    /// the canonical echo owns the message from then on, so a later page without it (a bounded
    /// window or a reopened scope) cannot revive the local copy.
    pub fn retire_echoed_sends(&mut self) {
        let threads = &self.threads;
        let echoed: Vec<Id> = self
            .commands
            .iter()
            .filter(|(_, command)| {
                command.method == corral_protocol::assistant::ASSISTANT_SEND
                    && command.status == CommandStatus::Done
                    && command.entry.as_ref().is_some_and(|entry| {
                        command.request["thread"]
                            .as_str()
                            .and_then(|thread| threads.get(thread))
                            .is_some_and(|thread| thread.entries.contains_key(entry))
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in echoed {
            self.commands.remove(&id);
            self.not_withdrawn.remove(&id);
        }
    }

    pub fn tick(&mut self, now: u64, clock: String) {
        self.now = now;
        self.clock = clock;
    }
}

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
}
