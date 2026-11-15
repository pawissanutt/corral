//! Shared native/browser application semantics; transports only carry CONTROL frames.
mod cache;
mod cancellation;
mod commands;
mod details;
mod devices;
mod events;
mod local;
mod rows;
mod scopes;
mod session_items;
mod terminal;
mod uploads;
use super::{
    core::{Event, topic_key},
    outbox::{self, Outbox},
    requests::{self, ClientRequest},
};
use crate::model::{Id, daemon::*, devices::Registry};
use corral_protocol::attachment::*;
use corral_protocol::device::{
    DEVICE_AUTHORITY_PREVIEW, DEVICE_PAGE, DEVICE_PAGE_LIMIT_MAX, DEVICE_REVOKE, DeviceInfo,
    DevicePage, DevicePageQuery, PUSH_STATUS, PUSH_SUBSCRIBE, PUSH_UNSUBSCRIBE, PUSH_V1,
    SESSION_AUTHORITY_CHANGED,
};
use corral_protocol::{
    self as wire,
    app::*,
    assistant::*,
    cmd::{CmdResult, CmdResultQuery, CmdState},
    detail::*,
    subscription::*,
};
use nsh_core::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type Outgoing = (&'static str, Value, Option<String>);
pub(super) const CAPS: &[&str] = &[
    wire::BOARD_V1,
    corral_protocol::device::DEVICE_V1,
    INBOX_V1,
    CHANGES_V1,
    PLANS_V1,
    AGENTS_V1,
    REVIEW_V1,
    TIMELINE_V1,
    USAGE_V1,
    SETTINGS_V1,
    ASSISTANT_V1,
    FILINGS_V1,
    ACT_V1,
    ASSISTANT_ATTACH_V1,
    SESSION_V1,
    corral_protocol::detail::STREAM_TERMINAL_V1,
    PUSH_V1,
];

pub(super) fn handles(event: &Event) -> bool {
    matches!(
        event,
        Event::Health(_)
            | Event::Board { .. }
            | Event::Rows { .. }
            | Event::Reset { .. }
            | Event::TopicError { .. }
            | Event::Live { .. }
    ) || matches!(event, Event::Notice { method, .. } if method == SESSION_AUTHORITY_CHANGED)
        || matches!(event, Event::Reply { method, .. } if matches!(*method,
            APP_SESSION | ACT_PREVIEW | ACT_SUBMIT | ACT_RESULT | DETAIL_GET | CHANGES_PAGE |
            SEEN_ADVANCE | DEVICE_SET_AUTHORITY | DEVICE_AUTHORITY_PREVIEW | DEVICE_REVOKE |
            wire::device::PAIR_BEGIN | wire::device::PAIR_STATUS | wire::device::PAIR_CONFIRM | wire::device::PAIR_CANCEL |
            PUSH_STATUS | PUSH_SUBSCRIBE | PUSH_UNSUBSCRIBE | ASSISTANT_SEND | ASSISTANT_WARM |
            ASSISTANT_STOP | ASSISTANT_SESSION | PROMPT_ANSWER | PROMPT_PREVIEW | DEVICE_PAGE |
            ASSISTANT_CANCEL | ATTACHMENT_BEGIN | ATTACHMENT_PUT | ATTACHMENT_DISCARD))
}

pub(super) struct AppState {
    ready: bool,
    namespace: Option<(String, String)>,
    outbox: Option<Outbox>,
    scopes: BTreeSet<String>,
    session_scopes: BTreeMap<String, usize>,
    terminal_scopes: BTreeMap<String, usize>,
    offline: super::offline::Cache,
    retained: cache::RetainedCache,
    detail_inflight: BTreeMap<String, String>,
    detail_pages: BTreeMap<String, Vec<DetailReply>>,
    detail_bytes: BTreeMap<String, usize>,
    detail_reread: BTreeMap<String, u64>,
    item_fetches: BTreeMap<String, session_items::ItemAssembly>,
    changes_inflight: bool,
    changes_cursor: Option<u64>,
    device_walk: Option<(String, Vec<DeviceInfo>)>,
    uploads: BTreeMap<String, uploads::Upload>,
    cancel_inflight: BTreeSet<String>,
}
impl AppState {
    pub fn new() -> Self {
        Self {
            ready: false,
            namespace: None,
            outbox: None,
            scopes: BTreeSet::from([QUICK_THREAD.into()]),
            session_scopes: BTreeMap::new(),
            retained: cache::RetainedCache::default(),
            terminal_scopes: BTreeMap::new(),
            offline: super::offline::Cache::default(),
            detail_inflight: BTreeMap::new(),
            detail_pages: BTreeMap::new(),
            detail_bytes: BTreeMap::new(),
            detail_reread: BTreeMap::new(),
            item_fetches: BTreeMap::new(),
            changes_inflight: false,
            changes_cursor: None,
            device_walk: None,
            uploads: BTreeMap::new(),
            cancel_inflight: BTreeSet::new(),
        }
    }
    pub fn welcomed(&mut self, feed: Signal<Feed>, welcome: &wire::Welcome) {
        self.ready = false;
        feed.update(|feed| {
            feed.mark_stale();
            feed.link = Link::Connecting;
            for thread in feed.threads.values_mut() {
                thread.live = None;
            }
            feed.capabilities = welcome.caps.iter().cloned().collect();
        });
    }
    pub fn disconnected(&mut self, feed: Signal<Feed>) {
        self.ready = false;
        self.changes_inflight = false;
        self.device_walk = None;
        self.detail_inflight.clear();
        self.detail_pages.clear();
        self.detail_bytes.clear();
        self.detail_reread.clear();
        self.cancel_inflight.clear();
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
            vec![open(Topic::Board, None), open(Topic::Health, None)]
        }
    }
    async fn session(
        &mut self,
        feed: Signal<Feed>,
        session: AppSession,
    ) -> Result<Vec<Outgoing>, String> {
        if self
            .namespace
            .as_ref()
            .is_some_and(|namespace| namespace.1 != session.device.id)
        {
            feed.update(|feed| feed.authority_revision = 0);
        }
        let store = feed.with_untracked(|feed| feed.store.clone().unwrap_or_default());
        let namespace = (store, session.device.id.clone());
        let same_namespace = self.namespace.as_ref() == Some(&namespace);
        let refresh_only = self.ready && same_namespace;
        if !same_namespace && self.namespace.is_some() {
            self.session_scopes.clear();
            self.terminal_scopes.clear();
            self.scopes = BTreeSet::from([QUICK_THREAD.into()]);
            self.uploads.clear();
            self.item_fetches.clear();
            self.retained = cache::RetainedCache::default();
            self.cancel_inflight.clear();
            feed.update(|feed| {
                feed.board = Board::default();
                feed.synced = None;
                feed.inbox.clear();
                feed.plans.clear();
                feed.agents.clear();
                feed.assistants.clear();
                feed.usage.clear();
                feed.settings.clear();
                feed.threads.clear();
                feed.details.clear();
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
                feed.cancelling.clear();
                feed.not_withdrawn.clear();
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
        let mut outgoing = vec![open(Topic::Board, None)];
        for (topic, cap) in [
            (Topic::Inbox, INBOX_V1),
            (Topic::Plans, PLANS_V1),
            (Topic::Agents, AGENTS_V1),
            (Topic::Assistant, ASSISTANT_V1),
            (Topic::Usage, USAGE_V1),
            (Topic::Settings, SETTINGS_V1),
        ] {
            if feed.with_untracked(|feed| feed.capable(cap)) {
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
        if feed.with_untracked(|feed| feed.capable(ASSISTANT_V1)) {
            for scope in &self.scopes {
                feed.update(|feed| {
                    let thread = feed.threads.entry(Id::from(scope)).or_default();
                    thread.loading = true;
                    thread.stale = true;
                    feed.topics.insert(
                        topic_key(Topic::Thread, Some(scope)),
                        TopicState {
                            loading: true,
                            stale: true,
                            error: None,
                        },
                    );
                });
                outgoing.push(open(Topic::Thread, Some(scope.clone())));
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
        if let Some(outbox) = &self.outbox {
            for id in outbox.saved.commands.keys() {
                if outbox
                    .saved
                    .commands
                    .get(id)
                    .is_some_and(|command| command.result.is_none() && command.error.is_none())
                {
                    outgoing.push(recover(id));
                }
            }
            // Message delivery is keyed by the original cmd_id, unlike command journal lookup.
            let messages: Vec<_> = outbox
                .saved
                .messages
                .values()
                .filter_map(outbox::Message::replay_request)
                .collect();
            for message in messages {
                outgoing.extend(self.request_async(feed, message).await);
            }
        }
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
    async fn settled(&mut self, feed: Signal<Feed>, id: &str) {
        let state = feed.with_untracked(|feed| feed.commands.get(id).cloned());
        if let Some(outbox) = &mut self.outbox {
            if let Err(problem) = outbox
                .update_async(|saved| {
                    if state
                        .as_ref()
                        .is_some_and(|state| state.status == CommandStatus::Done)
                        && !saved.messages.get(id).is_some_and(|message| message.cancel)
                    {
                        saved.messages.remove(id);
                    } else if let (Some(message), Some(state)) =
                        (saved.messages.get_mut(id), state.as_ref())
                    {
                        if !message.cancel
                            && matches!(
                                state.status,
                                CommandStatus::Refused | CommandStatus::Conflict
                            )
                        {
                            message.refused = true;
                            message.error = state
                                .error
                                .as_ref()
                                .map(|error| (error.code.clone(), error.code.clone()));
                        }
                    }
                    if let (Some(command), Some(state)) =
                        (saved.commands.get_mut(id), state.as_ref())
                    {
                        command.result = state.result.clone().map(|mut result| {
                            if let Some(error) = &mut result.error {
                                error.message = error.code.clone();
                            }
                            result
                        });
                        command.error = state
                            .error
                            .as_ref()
                            .map(|error| (error.code.clone(), error.code.clone()));
                    }
                })
                .await
            {
                feed.update(|feed| feed.problem = Some(problem));
            }
        }
        self.trim_retained(feed);
    }
    /// Native drains multiple intents before registration; browser dispatch/flush is synchronous.
    /// Drop prepared sends/uploads or filings reads retired by a later intent in the batch.
    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(super) fn filter_outgoing(&self, outgoing: &mut Vec<Outgoing>) {
        outgoing.retain(|(method, params, tag)| match *method {
            DETAIL_GET if params["kind"].as_str() == Some("filings") => tag
                .as_ref()
                .is_some_and(|id| self.detail_inflight.values().any(|current| current == id)),
            ASSISTANT_SEND => params["cmd_id"].as_str().is_some_and(|id| {
                self.outbox
                    .as_ref()
                    .and_then(|outbox| outbox.saved.messages.get(id))
                    .is_some_and(|message| !message.cancel)
            }),
            ATTACHMENT_BEGIN | ATTACHMENT_PUT => params["upload"].as_str().is_some_and(|id| {
                self.uploads
                    .get(id)
                    .is_some_and(|upload| upload.accepts(tag))
            }),
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
        result: None,
        error: None,
        entry: None,
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
fn recover(cmd_id: &str) -> Outgoing {
    out(
        ACT_RESULT,
        value(CmdResultQuery {
            cmd_id: cmd_id.into(),
            wait_ms: 1_000,
            abandon_if_absent: true,
        }),
        Some(cmd_id.into()),
    )
}

fn result_status(result: &CmdResult) -> CommandStatus {
    match result.state {
        CmdState::Done => CommandStatus::Done,
        CmdState::Failed => {
            if result.error.as_ref().is_some_and(|error| {
                error.code.starts_with("authority_")
                    || matches!(
                        error.code.as_str(),
                        "refused" | "revoked" | "admin_only" | "not_app_safe"
                    )
            }) {
                CommandStatus::Refused
            } else {
                CommandStatus::Failed
            }
        }
        CmdState::Conflict => CommandStatus::Conflict,
        CmdState::Running => CommandStatus::Running,
        CmdState::Busy => CommandStatus::Failed,
        CmdState::Abandoned => CommandStatus::Abandoned,
        _ => CommandStatus::Unknown,
    }
}
fn error_status(error: &ClientError) -> CommandStatus {
    if error.code.starts_with("authority_")
        || matches!(error.code.as_str(), "admin_only" | "not_app_safe")
    {
        return CommandStatus::Refused;
    }
    match error.code.as_str() {
        "conflict" => CommandStatus::Conflict,
        "refused" | "revoked" | "confirm_required" | "unauthenticated" => CommandStatus::Refused,
        "busy" => CommandStatus::Failed,
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

fn request_digest(method: &str, params: &Value) -> String {
    use sha2::{Digest, Sha256};
    // Store/device are bound by the outbox namespace. Keep canonical command values intact:
    // nested user data named `confirm` must not disappear with envelope proofs.
    let semantic = if method == ACT_SUBMIT {
        json!([method, params["command"], params["expect"]])
    } else {
        json!([method, params["key"], params["rev"], params["choice"]])
    };
    let encoded = serde_json::to_vec(&semantic).unwrap_or_default();
    format!("{:x}", Sha256::digest(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn same_id_confirmation_retry_ignores_freshness_but_not_command_input() {
        let mut request = json!({"cmd_id":"once","issued_at":"2026-11-14T10:00:00Z","expires_at":"2026-11-14T10:01:00Z","expect":{"revision":1},"command":{"family":"task","args":{"id":"T-1"}}});
        let digest = request_digest(ACT_SUBMIT, &request);
        request["confirm"] = json!({"preview_id":"temporary","digest":"proof"});
        request["issued_at"] = json!("2026-11-14T10:00:10Z");
        request["expires_at"] = json!("2026-11-14T10:01:10Z");
        assert_eq!(request_digest(ACT_SUBMIT, &request), digest);
        request["command"]["args"]["id"] = json!("T-2");
        assert_ne!(request_digest(ACT_SUBMIT, &request), digest);
    }

    #[test]
    fn reconnect_and_unenrolled_welcome_never_render_healthy() {
        use crate::model::{Form, devices::DeviceLink, effect::Conn, health};
        let health_reading = DaemonHealth {
            level: DaemonLevel::Healthy,
            reasons: vec![],
            runtimes: vec![],
            pm: PmLevel::Live,
            pass_seq: 1,
            last_pass: None,
            last_pass_ms: None,
            version: "1".into(),
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
            "authority":{"reply":true,"act":true,"assistant":true,"assistant_act":true,"admin":true},
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

    #[test]
    fn failed_stop_releases_the_turn_for_retry_without_claiming_abort() {
        let mut state = crate::model::assistant::State::default();
        let key = Id::from("failed-stop");
        state.live.stopping.insert("A-1".into(), 3);
        state.live.stop_ops.insert(key.clone(), ("A-1".into(), 3));
        let effects = state.settle_stop(
            &key,
            &Operation {
                method: ASSISTANT_STOP.into(),
                loading: false,
                value: None,
                error: Some(ClientError {
                    code: "refused".into(),
                    message: "Stop failed".into(),
                }),
            },
        );
        assert!(!state.live.stopping.contains_key("A-1"));
        assert!(state.live.stop_ops.is_empty());
        assert!(matches!(
            effects.as_slice(),
            [crate::model::Effect::Toast(_)]
        ));
    }
}

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
