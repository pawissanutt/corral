//! Owned daemon snapshots, independent of the transport and the UI runtime.
use crate::model::Id;
use corral_protocol::{
    app::*, assistant::ThreadEntry, cmd::CmdResult, detail::*, rows::*, subscription::LiveText,
};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    NeedsYou,
    Verifying,
    Producing,
    Queued,
    Held,
    Parked,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub mark: crate::model::board::Mark,
    pub sha7: Option<String>,
    /// Unix seconds.
    pub at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub session: Option<String>,
    pub role: String,
    pub profile: Option<String>,
    pub runtime: Option<String>,
    pub machine: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: Id,
    pub title: String,
    pub project: Option<String>,
    pub plan: Option<String>,
    pub goal: Option<String>,
    pub group: Group,
    /// Run, gate, review, land.
    pub rail: [Step; 4],
    pub now: String,
    pub now_code: String,
    /// Unix seconds since when `now` holds.
    pub since: Option<u64>,
    pub held_reason: Option<String>,
    pub questions: u64,
    pub attention: Vec<String>,
    pub agent: Option<Agent>,
    pub cost: Option<f64>,
    /// True when the cost is not wholly provider-reported.
    pub cost_estimated: bool,
    pub rev: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Board {
    pub epoch: u64,
    /// False until one complete snapshot has been applied since app startup.
    pub complete: bool,
    rows: Vec<Task>,
    pending_epoch: Option<u64>,
}

impl Board {
    pub fn tasks(&self) -> &[Task] {
        &self.rows
    }

    pub fn task(&self, id: &str) -> Option<&Task> {
        self.rows
            .binary_search_by(|task| order(&task.id, id))
            .ok()
            .map(|index| &self.rows[index])
    }

    pub fn in_group(&self, group: Group) -> impl Iterator<Item = &Task> {
        self.rows.iter().filter(move |task| task.group == group)
    }

    pub fn count(&self, group: Group) -> usize {
        self.in_group(group).count()
    }

    /// Keep the previous snapshot visible until the replacement is complete.
    pub fn begin_epoch(&mut self, epoch: u64) {
        self.pending_epoch = Some(epoch);
    }

    /// Apply only complete batches: page accumulation belongs to the session.
    pub fn apply_batch(&mut self, epoch: u64, rows: Vec<Task>, left: &[String]) {
        if !self.complete || self.epoch != epoch || self.pending_epoch == Some(epoch) {
            self.rows.clear();
        }
        for row in rows {
            match self
                .rows
                .binary_search_by(|existing| order(&existing.id, &row.id))
            {
                Ok(index) => self.rows[index] = row,
                Err(index) => self.rows.insert(index, row),
            }
        }
        if !left.is_empty() {
            let left: HashSet<&str> = left.iter().map(String::as_str).collect();
            self.rows.retain(|row| !left.contains(row.id.as_str()));
        }
        self.epoch = epoch;
        self.complete = true;
        self.pending_epoch = None;
    }
}

fn order(a: &str, b: &str) -> Ordering {
    fn number(id: &str) -> Option<u64> {
        id.strip_prefix("T-")?.parse().ok()
    }
    match (number(a), number(b)) {
        (Some(a_num), Some(b_num)) => a_num.cmp(&b_num).then_with(|| a.cmp(b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonLevel {
    Healthy,
    Degraded,
    Down,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeLevel {
    Ok,
    Unreachable,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmLevel {
    Live,
    Relaunching,
    Absent,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub kind: String,
    pub level: RuntimeLevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonHealth {
    pub level: DaemonLevel,
    pub reasons: Vec<String>,
    pub runtimes: Vec<Runtime>,
    pub pm: PmLevel,
    pub pass_seq: u64,
    /// Unix seconds of the last completed pass.
    pub last_pass: Option<u64>,
    pub last_pass_ms: Option<u64>,
    pub version: String,
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

#[derive(Debug, Clone, PartialEq)]
pub struct CommandState {
    pub device: Option<String>,
    pub method: String,
    pub request: serde_json::Value,
    pub status: CommandStatus,
    pub result: Option<CmdResult>,
    pub error: Option<ClientError>,
    /// The thread entry corrald acknowledged for an assistant send; kept with the command, never
    /// in the bounded reply cache, so the local copy stays merged into it.
    pub entry: Option<Id>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    pub method: String,
    pub loading: bool,
    pub value: Option<serde_json::Value>,
    pub error: Option<ClientError>,
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
    pub health: Option<DaemonHealth>,
    pub board: Board,
    pub session: Option<AppSession>,
    /// Current transport welcome, authenticated identity and projection readiness are distinct.
    pub transport_ready: bool,
    pub projection_ready: bool,
    pub capabilities: BTreeSet<String>,
    pub date: String,
    pub inbox: BTreeMap<Id, InboxRow>,
    pub plans: BTreeMap<Id, PlanRow>,
    pub agents: BTreeMap<Id, AgentRow>,
    pub assistants: BTreeMap<Id, AssistantRow>,
    pub usage: BTreeMap<Id, UsageRow>,
    pub settings: BTreeMap<Id, SettingsSnapshot>,
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
    pub changes: BTreeMap<u64, ChangeLine>,
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

    pub fn welcome_received(&mut self, store: String, health: DaemonHealth) {
        self.transport_ready = true;
        self.workspace = std::path::Path::new(&store)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .and_then(|parent| parent.to_str())
            .map(str::to_owned);
        self.store = Some(store);
        self.health_received(health);
    }

    pub fn health_received(&mut self, health: DaemonHealth) {
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
        self.inbox.clear();
        self.plans.clear();
        self.agents.clear();
        self.assistants.clear();
        self.usage.clear();
        self.settings.clear();
        self.threads.clear();
        self.details.clear();
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
    use crate::model::board::Mark;

    fn task(text: &str) -> Task {
        Task {
            id: Id::new(text),
            title: text.into(),
            project: None,
            plan: None,
            goal: None,
            group: Group::Queued,
            rail: std::array::from_fn(|_| Step {
                mark: Mark::Next,
                sha7: None,
                at: None,
            }),
            now: "queued".into(),
            now_code: "queued".into(),
            since: None,
            held_reason: None,
            questions: 0,
            attention: vec![],
            agent: None,
            cost: None,
            cost_estimated: false,
            rev: 1,
        }
    }

    #[test]
    fn board_batch_removes_left_and_keeps_numeric_order() {
        let mut board = Board::default();
        board.apply_batch(1, vec![task("T-10"), task("T-9"), task("other")], &[]);
        board.apply_batch(1, vec![task("T-2")], &["T-10".into()]);
        assert_eq!(
            board
                .tasks()
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["T-2", "T-9", "other"]
        );
        assert!(board.task("T-10").is_none());
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
