//! Shared native/browser application semantics; transports only carry CONTROL frames.
mod cancellation;
mod commands;
mod details;
mod events;
mod rows;
mod uploads;
use super::{
    core::{Event, topic_key},
    outbox::{self, Outbox},
    requests::{self, ClientRequest},
};
use crate::model::{Id, daemon::*, devices::Registry};
use corral_protocol::attachment::*;
use corral_protocol::device::{
    DEVICE_PAGE, DEVICE_PAGE_LIMIT_MAX, DeviceInfo, DevicePage, DevicePageQuery,
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
    ) || matches!(event, Event::Reply { method, .. } if matches!(*method,
            APP_SESSION | ACT_PREVIEW | ACT_SUBMIT | ACT_RESULT | DETAIL_GET | CHANGES_PAGE |
            SEEN_ADVANCE | DEVICE_SET_AUTHORITY | ASSISTANT_SEND | ASSISTANT_WARM |
            ASSISTANT_STOP | ASSISTANT_SESSION | PROMPT_ANSWER | PROMPT_PREVIEW | DEVICE_PAGE |
            ASSISTANT_CANCEL | ATTACHMENT_BEGIN | ATTACHMENT_PUT | ATTACHMENT_DISCARD))
}

pub(super) struct AppState {
    ready: bool,
    namespace: Option<(String, String)>,
    outbox: Option<Outbox>,
    scopes: BTreeSet<String>,
    detail_inflight: BTreeMap<String, String>,
    detail_pages: BTreeMap<String, Vec<DetailReply>>,
    detail_bytes: BTreeMap<String, usize>,
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
            detail_inflight: BTreeMap::new(),
            detail_pages: BTreeMap::new(),
            detail_bytes: BTreeMap::new(),
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
    fn session(
        &mut self,
        feed: Signal<Feed>,
        session: AppSession,
    ) -> Result<Vec<Outgoing>, String> {
        let store = feed.with_untracked(|feed| feed.store.clone().unwrap_or_default());
        let namespace = (store, session.device.id.clone());
        let same_namespace = self.namespace.as_ref() == Some(&namespace);
        let refresh_only = self.ready && same_namespace;
        if !same_namespace && self.namespace.is_some() {
            self.scopes = BTreeSet::from([QUICK_THREAD.into()]);
            self.uploads.clear();
            self.cancel_inflight.clear();
            feed.update(|feed| {
                feed.board = Board::default();
                feed.inbox.clear();
                feed.plans.clear();
                feed.agents.clear();
                feed.assistants.clear();
                feed.usage.clear();
                feed.settings.clear();
                feed.threads.clear();
                feed.details.clear();
                // A registry listed after this connection's welcome already belongs to the new
                // namespace; anything older does not.
                if feed.registry != Registry::Current {
                    feed.devices.clear();
                }
                feed.changes.clear();
                feed.changes_head = 0;
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
            match Outbox::load(&namespace.0, &namespace.1) {
                Ok(outbox) => self.outbox = Some(outbox),
                Err(problem) => feed.update(|feed| feed.problem = Some(problem)),
            }
        }
        self.ready = true;
        feed.update(|feed| {
            // Until the list arrives, show this device alone; it does not make the list current.
            if feed.devices.is_empty() && feed.registry != Registry::Current {
                feed.devices = vec![super::map::device(session.device.clone())];
            }
            feed.seen = Some(session.seen.clone());
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
        outgoing.extend(self.poll_changes(feed));
        if let Some(outbox) = &self.outbox {
            feed.update(|feed| {
                feed.drafts = outbox
                    .saved
                    .drafts
                    .iter()
                    .map(|(key, value)| (Id::from(key), value.clone()))
                    .collect();
                for (id, command) in &outbox.saved.commands {
                    let mut state = command_state(
                        feed,
                        &command.method,
                        command.params.clone(),
                        CommandStatus::Unknown,
                    );
                    state.result = command.result.clone();
                    state.error = command.error.as_ref().map(|(code, message)| ClientError {
                        code: code.clone(),
                        message: message.clone(),
                    });
                    if let Some(result) = &state.result {
                        state.status = result_status(result);
                    } else if let Some(error) = &state.error {
                        state.status = error_status(error);
                    }
                    feed.commands.entry(Id::from(id)).or_insert(state);
                }
                for (id, message) in &outbox.saved.messages {
                    let mut state = command_state(
                        feed,
                        ASSISTANT_SEND,
                        serde_json::to_value(&message.request).unwrap_or(Value::Null),
                        if message.attempted {
                            CommandStatus::Unknown
                        } else {
                            CommandStatus::Queued
                        },
                    );
                    if let Some((code, explanation)) = &message.error {
                        state.error = Some(ClientError {
                            code: code.clone(),
                            message: explanation.clone(),
                        });
                        state.status = if message.refused {
                            CommandStatus::Refused
                        } else {
                            CommandStatus::Unknown
                        };
                    }
                    feed.commands.entry(Id::from(id)).or_insert(state);
                    if message.cancel {
                        feed.cancelling.insert(Id::from(id));
                        if let Some(state) = feed.commands.get_mut(id.as_str()) {
                            state.status = CommandStatus::Unknown;
                        }
                    }
                }
            });
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
                outgoing.extend(self.request(feed, message));
            }
        }
        let targets = feed.with_untracked(|feed| {
            feed.details
                .values()
                .map(|detail| detail.target.clone())
                .collect::<Vec<_>>()
        });
        for target in targets {
            let etag = feed.with_untracked(|feed| {
                feed.detail(&target)
                    .and_then(|detail| detail.pages.first())
                    .map(|page| page.etag.clone())
            });
            outgoing.extend(self.request(
                feed,
                ClientRequest::Detail {
                    request_id: requests::new_id(),
                    request: DetailGet {
                        target,
                        etag,
                        page: None,
                    },
                },
            ));
        }
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
    fn settled(&mut self, feed: Signal<Feed>, id: &str) {
        let state = feed.with_untracked(|feed| feed.commands.get(id).cloned());
        if let Some(outbox) = &mut self.outbox {
            if let Err(problem) = outbox.update(|saved| {
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
                if let (Some(command), Some(state)) = (saved.commands.get_mut(id), state) {
                    command.result = state.result.map(|mut result| {
                        if let Some(error) = &mut result.error {
                            error.message = error.code.clone();
                        }
                        result
                    });
                    command.error = state.error.map(|error| (error.code.clone(), error.code));
                }
            }) {
                feed.update(|feed| feed.problem = Some(problem));
            }
        }
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
fn without_proofs(mut value: Value) -> Value {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(object) => {
                object.remove("confirm");
                object.remove("operation_token");
                for value in object.values_mut() {
                    strip(value);
                }
            }
            Value::Array(array) => {
                for value in array {
                    strip(value);
                }
            }
            _ => {}
        }
    }
    strip(&mut value);
    value
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

fn request_digest(params: &Value) -> String {
    use sha2::{Digest, Sha256};
    let encoded = serde_json::to_vec(&without_proofs(params.clone())).unwrap_or_default();
    format!("{:x}", Sha256::digest(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_identity_ignores_short_lived_proofs_but_not_command_input() {
        let mut request = json!({"cmd_id":"once","command":{"family":"task","args":{"id":"T-1"}}});
        let digest = request_digest(&request);
        request["confirm"] = json!({"preview_id":"temporary","digest":"proof"});
        assert_eq!(request_digest(&request), digest);
        request["command"]["args"]["id"] = json!("T-2");
        assert_ne!(request_digest(&request), digest);
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
