//! Shared native/browser application semantics; transports only carry CONTROL frames.
mod cache;
mod commands;
mod details;
mod devices;
mod events;
mod html;
mod local;
mod rows;
mod scopes;
mod session_items;
mod terminal;
#[path = "app/details/transcript_window.rs"]
mod transcript_window;
mod uploads;
use super::{
    core::{Event, topic_key},
    outbox::{self, Outbox},
    requests::{self, ClientRequest},
};
use crate::model::{Id, daemon::*, devices::Registry};
use corral_protocol::attachment::*;
use corral_protocol::build::{BUILD_REMOVE, BUILD_START};
use corral_protocol::config::{
    CONFIG_GET, CONFIG_PUT, PI_HOSTED_TOOL_MODELS, PI_HOSTED_TOOLS, PI_STARTUP,
};
use corral_protocol::device::{
    DEVICE_PAGE, DEVICE_PAGE_LIMIT_MAX, DEVICE_REVOKE, DeviceInfo, DevicePage, DevicePageQuery,
    PUSH_STATUS, PUSH_SUBSCRIBE, PUSH_UNSUBSCRIBE,
};
use corral_protocol::op::{OP_RESULT, OP_SUBMIT};
use corral_protocol::session as control;
use corral_protocol::session_host::{
    SESSION_ACTION, SESSION_CARD_ANSWER, SESSION_MODEL, SESSION_QUEUE_MOVE, SESSION_QUEUE_WITHDRAW,
    SESSION_SEND, SessionSend,
};
use corral_protocol::{self as wire, Refusal, app::*, assistant::*, detail::*, subscription::*};
use nsh_core::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Outgoing = (&'static str, Value, Option<String>);
pub(super) const CAPS: &[&str] = &[
    wire::TASKS_V1,
    corral_protocol::device::DEVICE_V1,
    CHANGES_V1,
    PLANS_V1,
    PROJECTS_V1,
    AGENTS_V1,
    REVIEW_V1,
    TIMELINE_V1,
    SYSTEM_V1,
    SYSTEM_RUNTIME_V1,
    SYSTEM_ENTRIES_V1,
    SYSTEM_LENSES_V1,
    SYSTEM_DATA_V1,
    corral_protocol::host::HOST_V1,
    USAGE_V1,
    SETTINGS_V1,
    AUTOMATION_V1,
    corral_protocol::config::PI_HOSTED_TOOLS_V1,
    corral_protocol::config::PI_HOSTED_TOOL_MODELS_V1,
    corral_protocol::config::PI_STARTUP_V1,
    ASSISTANT_V1,
    CLOSED_V1,
    CITATIONS_V1,
    ASSISTANT_ATTACH_V1,
    SESSION_V1,
    SESSION_FLEET_V1,
    STREAM_TERMINAL_V1,
    #[cfg(not(windows))]
    corral_protocol::device::PUSH_V1,
    SESSION_STATE_V1,
    SESSION_FILES_V1,
    SESSION_HTML_V1,
    BUILD_V1,
];

pub(super) fn handles(event: &Event) -> bool {
    matches!(
        event,
        Event::Health(_) | Event::Rows { .. } | Event::Reset { .. } | Event::TopicError { .. }
    ) || matches!(event, Event::Reply { method, .. } if matches!(*method,
            APP_SESSION | OP_SUBMIT | OP_RESULT | CONFIG_GET | CONFIG_PUT | PI_HOSTED_TOOLS | PI_HOSTED_TOOL_MODELS | PI_STARTUP | SESSION_SEND | BUILD_START | wire::build::CODE_START | wire::build::CODE_RESUME |
            SESSION_CARD_ANSWER | SESSION_QUEUE_WITHDRAW | SESSION_QUEUE_MOVE | SESSION_ACTION |
            SESSION_MODEL | BUILD_REMOVE | DETAIL_GET | CHANGES_PAGE | SEEN_ADVANCE | DEVICE_REVOKE |
            wire::device::PAIR_BEGIN | wire::device::PAIR_STATUS | wire::device::PAIR_CONFIRM | wire::device::PAIR_CANCEL |
            PUSH_STATUS | PUSH_SUBSCRIBE | PUSH_UNSUBSCRIBE | ASSISTANT_WARM |
            ASSISTANT_SESSION | DEVICE_PAGE |
            ATTACHMENT_BEGIN | ATTACHMENT_PUT | ATTACHMENT_DISCARD |
            control::SESSION_CANCEL | control::SESSION_PAUSE | control::SESSION_RESUME |
            control::SESSION_TERMINATE | control::SESSION_RELAUNCH | control::SESSION_REWIND))
}

/// Whether a refusal of `method` carries a typed `Refusal` the app shows with its reason.
fn refusable(method: &str) -> bool {
    matches!(
        method,
        OP_SUBMIT
            | OP_RESULT
            | CONFIG_PUT
            | SESSION_CARD_ANSWER
            | SESSION_QUEUE_WITHDRAW
            | SESSION_QUEUE_MOVE
            | SESSION_ACTION
            | SESSION_MODEL
            | BUILD_REMOVE
            | control::SESSION_CANCEL
            | control::SESSION_PAUSE
            | control::SESSION_RESUME
            | control::SESSION_TERMINATE
            | control::SESSION_RELAUNCH
            | control::SESSION_REWIND
            | SESSION_SEND
            | BUILD_START
            | wire::build::CODE_START
            | wire::build::CODE_RESUME
    )
}

pub(super) struct AppState {
    ready: bool,
    namespace: Option<(String, String)>,
    outbox: Option<Outbox>,
    session_scopes: BTreeMap<String, usize>,
    terminal_scopes: BTreeMap<String, usize>,
    offline: super::offline::Cache,
    retained: cache::RetainedCache,
    #[cfg(test)]
    test_reader_limit: Option<usize>,
    shared_holders: BTreeMap<u64, String>,
    reader_windows: BTreeMap<u64, transcript_window::Window>,
    detail_inflight: BTreeMap<String, String>,
    detail_pages: BTreeMap<String, Vec<DetailReply>>,
    detail_bytes: BTreeMap<String, usize>,
    detail_reread: BTreeMap<String, u64>,
    /// Explicit newest-only transcript reads, distinct from depth-preserving refreshes.
    detail_latest: BTreeSet<String>,
    /// Mounted transcript views that are away from the end, by exact session scope.
    transcript_readers: BTreeMap<String, usize>,
    /// Depth added by live reconciliation, rather than by an explicit older-page request.
    transcript_auto_depth: BTreeSet<String>,
    /// A child transcript changed during a read and needs one follow-up when it settles.
    transcript_deferred: BTreeSet<String>,
    /// How many times in a row each detail's read started again from its first page because it
    /// changed between two of its pages.
    detail_restarts: BTreeMap<String, u8>,
    item_fetches: BTreeMap<String, session_items::ItemAssembly>,
    changes_inflight: bool,
    changes_cursor: Option<u64>,
    device_walk: Option<(String, Vec<DeviceInfo>)>,
    uploads: BTreeMap<String, uploads::Upload>,
    html: Option<html::HtmlTransfer>,
}
impl AppState {
    pub fn new() -> Self {
        Self {
            ready: false,
            namespace: None,
            outbox: None,
            session_scopes: BTreeMap::new(),
            retained: cache::RetainedCache::default(),
            terminal_scopes: BTreeMap::new(),
            offline: super::offline::Cache::default(),
            #[cfg(test)]
            test_reader_limit: None,
            shared_holders: BTreeMap::new(),
            reader_windows: BTreeMap::new(),
            detail_inflight: BTreeMap::new(),
            detail_pages: BTreeMap::new(),
            detail_bytes: BTreeMap::new(),
            detail_reread: BTreeMap::new(),
            detail_latest: BTreeSet::new(),
            transcript_readers: BTreeMap::new(),
            transcript_auto_depth: BTreeSet::new(),
            transcript_deferred: BTreeSet::new(),
            detail_restarts: BTreeMap::new(),
            item_fetches: BTreeMap::new(),
            changes_inflight: false,
            changes_cursor: None,
            device_walk: None,
            uploads: BTreeMap::new(),
            html: None,
        }
    }
    pub fn welcomed(&mut self, feed: Signal<Feed>, welcome: &wire::Welcome) {
        self.interrupt_html();
        self.interrupt_readers(feed);
        self.ready = false;
        feed.update(|feed| {
            feed.mark_stale();
            feed.link = Link::Connecting;
            feed.capabilities = welcome.caps.iter().cloned().collect();
        });
    }
    pub fn disconnected(&mut self, feed: Signal<Feed>) {
        self.interrupt_html();
        self.interrupt_readers(feed);
        self.ready = false;
        self.changes_inflight = false;
        self.device_walk = None;
        self.detail_inflight.clear();
        self.detail_pages.clear();
        self.detail_bytes.clear();
        self.detail_reread.clear();
        self.detail_latest.clear();
        self.transcript_deferred.clear();
        self.detail_restarts.clear();
        for upload in self.uploads.values_mut() {
            upload.disconnected();
        }
        feed.update(Feed::mark_stale);
    }
    pub fn ready(&mut self, feed: Signal<Feed>) -> Vec<Outgoing> {
        self.ready = false;
        if feed.with_untracked(|feed| CAPS.iter().skip(2).any(|cap| feed.capable(cap))) {
            vec![out(APP_SESSION, json!({}), None), open(Topic::Health, None)]
        } else {
            vec![open(Topic::Tasks, None), open(Topic::Health, None)]
        }
    }
    async fn session(
        &mut self,
        feed: Signal<Feed>,
        session: AppSession,
    ) -> Result<Vec<Outgoing>, String> {
        let store = feed.with_untracked(|feed| feed.store.clone().unwrap_or_default());
        let namespace = (store, session.device.id.clone());
        let same_namespace = self.namespace.as_ref() == Some(&namespace);
        let refresh_only = self.ready && same_namespace;
        if !same_namespace && self.namespace.is_some() {
            self.interrupt_html();
            self.session_scopes.clear();
            self.terminal_scopes.clear();
            self.transcript_readers.clear();
            self.reader_windows.clear();
            self.shared_holders.clear();
            self.transcript_auto_depth.clear();
            self.uploads.clear();
            self.item_fetches.clear();
            self.retained = cache::RetainedCache::default();
            feed.update(|feed| {
                feed.board = Board::default();
                feed.synced = None;
                feed.plans.clear();
                feed.projects.clear();
                feed.systems.clear();
                feed.agents.clear();
                feed.assistants.clear();
                feed.usage = None;
                feed.settings = None;
                feed.daemon_host = None;
                feed.daemon_host_at = None;
                feed.details.clear();
                feed.transcript_windows.clear();
                feed.items.clear();
                feed.live.clear();
                // A registry listed after this connection's welcome already belongs to the new
                // namespace; anything older does not.
                if feed.registry != Registry::Current {
                    feed.devices.clear();
                }
                feed.changes.clear();
                feed.changes_head = 0;
                feed.changes_unseen = None;
                feed.changes_older = None;
                feed.topics.clear();
                feed.operations.clear();
                feed.uploads.clear();
            });
        }
        if !same_namespace || self.outbox.is_none() {
            self.outbox = None;
            self.namespace = Some(namespace.clone());
            self.changes_cursor = None;
            feed.update(|feed| {
                feed.commands.clear();
                feed.drafts.clear();
            });
            match Outbox::load_async(&namespace.0, &namespace.1).await {
                Ok(outbox) => self.outbox = Some(outbox),
                Err(problem) => feed.update(|feed| feed.problem = Some(problem)),
            }
        }
        if let Err(problem) = self.offline.authenticated(feed, &session).await {
            self.ready = false;
            self.outbox = None;
            return Err(problem);
        }
        self.ready = true;
        feed.update(|feed| {
            // Until the list arrives, show this device alone; it does not make the list current.
            if feed.devices.is_empty() && feed.registry != Registry::Current {
                feed.devices = vec![super::map::device(session.device.clone())];
            }
            feed.seen_received(session.seen.clone());
            feed.session = Some(session);
        });
        if refresh_only {
            return Ok(vec![]);
        }
        let mut outgoing = vec![open(Topic::Tasks, None)];
        for (topic, cap) in [
            (Topic::Plans, PLANS_V1),
            (Topic::Projects, PROJECTS_V1),
            (Topic::System, SYSTEM_V1),
            (Topic::Agents, AGENTS_V1),
            (Topic::Assistant, ASSISTANT_V1),
            (Topic::Usage, USAGE_V1),
            (Topic::Settings, SETTINGS_V1),
            (Topic::Host, corral_protocol::host::HOST_V1),
        ] {
            // Build sessions are rows of the assistant topic: it opens for either grant.
            let granted = feed.with_untracked(|feed| {
                feed.capable(cap) || (topic == Topic::Assistant && feed.capable(BUILD_V1))
            });
            if granted {
                feed.update(|feed| {
                    feed.topics.insert(
                        topic_key(topic, None),
                        TopicState {
                            loading: true,
                            stale: true,
                            error: None,
                        },
                    );
                });
                outgoing.push(open(topic, None));
            }
        }
        if feed.with_untracked(|feed| feed.capable(SESSION_V1)) {
            for scope in self.session_scopes.keys() {
                feed.update(|feed| {
                    feed.topics.insert(
                        topic_key(Topic::Session, Some(scope)),
                        TopicState {
                            loading: true,
                            stale: true,
                            error: None,
                        },
                    );
                });
                outgoing.push(open(Topic::Session, Some(scope.clone())));
            }
        }
        if feed.with_untracked(|feed| feed.capable(STREAM_TERMINAL_V1)) {
            for scope in self.terminal_scopes.keys() {
                outgoing.push(open(Topic::Terminal, Some(scope.clone())));
            }
        }
        self.project_local(feed);
        outgoing.extend(self.resume_items());
        outgoing.extend(self.poll_changes(feed));
        outgoing.extend(self.recover_messages(feed).await);
        outgoing.extend(self.refresh_details(feed));
        outgoing.extend(self.pump_uploads());
        Ok(outgoing)
    }
    fn poll_changes(&mut self, feed: Signal<Feed>) -> Vec<Outgoing> {
        if !self.ready
            || self.changes_inflight
            || !feed.with_untracked(|feed| feed.capable(CHANGES_V1))
        {
            return vec![];
        }
        self.changes_inflight = true;
        self.operation(
            feed,
            requests::new_id(),
            CHANGES_PAGE,
            value(ChangesQuery {
                before: None,
                after: self.changes_cursor,
                limit: 200,
            }),
        )
    }
    /// corrald answered message `id` (or refused it): its outcome is the operation's from now
    /// on, so the outbox lets it go.
    async fn settled(&mut self, feed: Signal<Feed>, id: &str) {
        if let Some(outbox) = &mut self.outbox
            && outbox.saved.messages.contains_key(id)
            && let Err(problem) = outbox
                .update_async(|saved| {
                    saved.messages.remove(id);
                })
                .await
        {
            feed.update(|feed| feed.problem = Some(problem));
        }
        self.trim_retained(feed);
    }
    /// On every authenticated connection: a message that never left this device goes now,
    /// under its own id. One that left without an answer is not sent again, as the host takes
    /// an input id once and it may have arrived: the outbox lets it go, its words go back to its
    /// session's saved draft unless one is there, and its send says the connection dropped
    /// before corrald answered.
    async fn recover_messages(&mut self, feed: Signal<Feed>) -> Vec<Outgoing> {
        let Some(outbox) = self.outbox.as_mut() else {
            return vec![];
        };
        let lost: Vec<SessionSend> = outbox
            .saved
            .messages
            .values()
            .filter(|message| message.attempted)
            .map(|message| message.request.clone())
            .collect();
        let waiting: Vec<String> = outbox
            .saved
            .messages
            .iter()
            .filter(|(_, message)| !message.attempted)
            .map(|(id, _)| id.clone())
            .collect();
        if !lost.is_empty() {
            let kept = outbox
                .update_async(|saved| {
                    for request in &lost {
                        saved.messages.remove(&request.cmd_id);
                        let draft = saved.drafts.entry(request.id.clone()).or_default();
                        if draft.trim().is_empty() {
                            *draft = request.text.clone();
                        }
                    }
                })
                .await;
            let drafts = outbox.saved.drafts.clone();
            feed.update(|feed| match &kept {
                Ok(()) => {
                    for request in &lost {
                        if let Some(draft) = drafts.get(&request.id) {
                            feed.drafts
                                .insert(Id::from(request.id.as_str()), draft.clone());
                        }
                        if let Some(state) = feed.commands.get_mut(request.cmd_id.as_str()) {
                            state.status = CommandStatus::Unknown;
                        }
                        let told = feed
                            .operations
                            .get(request.cmd_id.as_str())
                            .is_some_and(|operation| !operation.loading);
                        if !told {
                            feed.operations.insert(
                                Id::from(request.cmd_id.as_str()),
                                Operation {
                                    method: SESSION_SEND.into(),
                                    loading: false,
                                    value: None,
                                    error: Some(send_lost()),
                                    refusal: None,
                                },
                            );
                        }
                    }
                }
                Err(problem) => feed.problem = Some(problem.clone()),
            });
        }
        let mut outgoing = vec![];
        for id in waiting {
            outgoing.extend(self.replay(feed, &id).await);
        }
        outgoing
    }
    /// Sends kept message `id` for the first time, unless another instance settled or sent it.
    async fn replay(&mut self, feed: Signal<Feed>, id: &str) -> Vec<Outgoing> {
        let Some(outbox) = self.outbox.as_mut() else {
            return vec![];
        };
        let mut request = None;
        let marked = outbox
            .update_checked(|saved| match saved.messages.get_mut(id) {
                Some(message) if !message.attempted => {
                    message.attempted = true;
                    request = Some(message.request.clone());
                    Ok(())
                }
                _ => Err("settled elsewhere".into()),
            })
            .await;
        let (Ok(()), Some(request)) = (marked, request) else {
            return vec![];
        };
        let params = value(&request);
        feed.update(|feed| {
            if let Some(state) = feed.commands.get_mut(id) {
                state.status = CommandStatus::Pending;
            }
            trim_operations(feed);
            feed.operations.insert(
                Id::from(id),
                Operation {
                    method: SESSION_SEND.into(),
                    loading: true,
                    value: None,
                    error: None,
                    refusal: None,
                },
            );
        });
        vec![out(SESSION_SEND, params, Some(id.to_owned()))]
    }
    /// Native drains multiple intents before registration; browser dispatch/flush is synchronous.
    /// Drop prepared sends or uploads retired by a later intent in the batch.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn filter_outgoing(&self, outgoing: &mut Vec<Outgoing>) {
        outgoing.retain(|(method, params, tag)| match *method {
            SESSION_SEND => params["cmd_id"].as_str().is_some_and(|id| {
                self.outbox
                    .as_ref()
                    .and_then(|outbox| outbox.saved.messages.get(id))
                    .is_some()
            }),
            ATTACHMENT_BEGIN | ATTACHMENT_PUT => params["upload"].as_str().is_some_and(|id| {
                self.uploads
                    .get(id)
                    .is_some_and(|upload| upload.accepts(tag))
            }),
            DETAIL_GET
                if tag
                    .as_deref()
                    .is_some_and(|tag| tag.starts_with(transcript_window::PREFIX)) =>
            {
                tag.as_deref().is_some_and(|tag| self.reader_accepts(tag))
            }
            DETAIL_GET
                if tag
                    .as_deref()
                    .is_some_and(|tag| tag.starts_with(html::TAG_PREFIX)) =>
            {
                tag.as_deref().is_some_and(|tag| {
                    self.html
                        .as_ref()
                        .is_some_and(|transfer| transfer.accepts(tag))
                })
            }
            _ => true,
        });
    }
}
fn command_state(feed: &Feed, method: &str, request: Value, status: CommandStatus) -> CommandState {
    CommandState {
        device: feed
            .session
            .as_ref()
            .map(|session| session.device.id.clone())
            .or_else(|| match &feed.device {
                crate::model::devices::DeviceLink::Enrolled { id, .. } => Some(id.clone()),
                _ => None,
            }),
        method: method.into(),
        request,
        status,
        error: None,
    }
}
fn trim_operations(feed: &mut Feed) {
    while feed.operations.len() >= 512 {
        let key = feed
            .operations
            .iter()
            .find(|(_, operation)| !operation.loading)
            .map(|(id, _)| id.clone());
        if let Some(key) = key {
            feed.operations.remove(&key);
        } else {
            break;
        }
    }
}
fn offline() -> ClientError {
    ClientError {
        code: "offline".into(),
        message: "Not sent: the authenticated daemon is offline.".into(),
    }
}
fn local_error(message: String) -> ClientError {
    ClientError {
        code: "local_storage".into(),
        message,
    }
}
/// A send whose connection dropped before corrald answered it.
fn send_lost() -> ClientError {
    ClientError {
        code: "disconnected".into(),
        message: SEND_LOST.into(),
    }
}
fn value(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("invalid app reply: {error}"))
}
fn out(method: &'static str, params: Value, tag: Option<String>) -> Outgoing {
    (method, params, tag)
}
fn open(topic: Topic, scope: Option<String>) -> Outgoing {
    out(
        wire::SUB_OPEN,
        value(SubOpen {
            topic,
            since: None,
            scope,
        }),
        None,
    )
}
/// A send refused on the connection; anything else may have run.
fn error_status(error: &ClientError) -> CommandStatus {
    match error.code.as_str() {
        "conflict" => CommandStatus::Conflict,
        "refused" | "revoked" | "unauthenticated" | "invalid_params" => CommandStatus::Refused,
        _ => CommandStatus::Failed,
    }
}

pub(super) fn devices() -> Outgoing {
    out(
        DEVICE_PAGE,
        value(DevicePageQuery {
            after: None,
            limit: DEVICE_PAGE_LIMIT_MAX,
        }),
        Some(requests::new_id()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_and_unenrolled_welcome_never_render_healthy() {
        use crate::model::{Form, devices::DeviceLink, effect::Conn, health};
        let health_reading = wire::Health {
            daemon: wire::DaemonState::Healthy,
            runner_reachable: true,
            last_sync_at: None,
            last_sync_error: None,
            pid: 1,
            version: "1".into(),
            slots: None,
        };
        let mut feed = Feed::default();
        feed.welcome_received("/readiness/.corral".into(), health_reading.clone());
        for identity in [
            DeviceLink::Unenrolled,
            DeviceLink::Failed("credential read failed".into()),
        ] {
            feed.device = identity;
            assert_eq!(health::daemon_env(&feed, Form::Terminal).conn, Conn::Down);
            assert!(
                health::daemon_snapshot(&feed, Form::Terminal)
                    .health
                    .segments
                    .iter()
                    .all(|segment| segment.tone != health::Tone::Ok)
            );
        }
        feed.device = DeviceLink::Enrolled {
            id: "dev-ready".into(),
            name: "Ready".into(),
        };
        feed.session = Some(serde_json::from_value(json!({
            "device":{"id":"dev-ready","name":"Ready","platform":"native","app_version":"0","enrolled_at":"2026-11-14T09:00:00Z","revoked_at":null},
            "seen":{"through":0,"at":null},"server_time":"2026-11-14T09:00:00Z"
        })).unwrap());
        feed.board_received(1, vec![], &[], true);
        assert_eq!(
            health::daemon_env(&feed, Form::Terminal).conn,
            Conn::Healthy
        );
        feed.link_lost("connection interrupted".into());
        feed.connecting("test".into());
        assert!(!feed.ready());
        assert_eq!(health::daemon_env(&feed, Form::Terminal).conn, Conn::Down);
        assert!(health::daemon_connection_banner(&feed, Form::Terminal).is_some());
        assert!(feed.board.complete);
        assert_eq!(feed.problem.as_deref(), Some("connection interrupted"));
    }

    #[test]
    fn offline_new_end_and_profile_never_become_phantom_pending_operations() {
        use crate::model::assistant::{Action, State};
        let feed = Feed::default();
        let mut state = State::default();
        for action in [
            Action::NewSession,
            Action::ConfirmEnd("A-1".into()),
            Action::PickProfile {
                thread: "A-1".into(),
                profile: "other".into(),
            },
        ] {
            let effects = state.act_live(action, &feed);
            assert!(matches!(
                effects.as_slice(),
                [crate::model::Effect::Toast(_)]
            ));
            assert!(state.live.waiting_ops().is_empty());
        }
        assert!(state.live.picks.is_empty());
        assert!(state.live.ends.is_empty());
    }
}

#[cfg(test)]
#[path = "app/control_tests.rs"]
mod control_tests;

fn trim_commands(feed: Signal<Feed>) {
    feed.update(|feed| {
        while feed.commands.len() >= 512 {
            let settled = feed
                .commands
                .iter()
                .find(|(_, state)| {
                    matches!(
                        state.status,
                        CommandStatus::Done
                            | CommandStatus::Failed
                            | CommandStatus::Refused
                            | CommandStatus::Conflict
                            | CommandStatus::Abandoned
                    )
                })
                .map(|(id, _)| id.clone());
            let Some(settled) = settled else {
                break;
            };
            feed.commands.remove(&settled);
        }
    });
}
