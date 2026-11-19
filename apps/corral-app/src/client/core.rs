/// Plain request deadlines; reads may wait on the daemon's reader worker first.
const APP_REQUEST_DEADLINE_MS: u64 = 5_000;
const APP_REQUEST_MARGIN_MS: u64 = 5_000;
const APP_READ_WAIT_MS: u64 = 25_000;
const APP_READ_DEADLINE_MS: u64 = APP_READ_WAIT_MS + APP_REQUEST_MARGIN_MS;
/// The daemon answers a long operation with `running` after 5 s; this leaves it room.
const APP_COMMAND_DEADLINE_MS: u64 = 25_000 + APP_REQUEST_MARGIN_MS;
use corral_protocol::frame::{Frame, FrameKind};
#[cfg(test)]
use corral_protocol::rpc::Request;
use corral_protocol::rpc::{ErrorCode, JSONRPC, Notification, Response, ResponseOutcome};
use corral_protocol::subscription::{SubAck, SubClose, SubDelta, SubOpen, SubReset, Topic};
use corral_protocol::{self as wire, ClientInfo, Health, Hello, Proto, Welcome};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Error code of a reply the client synthesized because corrald's answer never arrived in time;
/// the request may still have taken effect.
pub(super) const DEADLINE_EXPIRED: &str = "deadline_expired";
const MAX_TOPIC_RESET_ERRORS: u8 = 3;

pub(super) enum Event {
    Health(Health),
    Reply {
        tag: Option<String>,
        params: Value,
        method: &'static str,
        outcome: Result<Value, (ErrorCode, String)>,
        error_data: Option<Value>,
    },
    Rows {
        delta: SubDelta,
        replacement: bool,
    },
    Reset {
        topic: Topic,
        scope: Option<String>,
        epoch: u64,
    },
    TopicError {
        topic: Topic,
        scope: Option<String>,
        code: ErrorCode,
        message: String,
    },
    Notice {
        method: String,
        params: Value,
    },
    #[cfg(not(target_arch = "wasm32"))]
    Idle,
    #[cfg(not(target_arch = "wasm32"))]
    Closed(String),
}

struct Pending {
    method: &'static str,
    tag: Option<String>,
    params: Value,
    deadline: Option<u64>,
    closing: Option<String>,
}

/// The identity a topic row is keyed by in `RowsDelta.left`: the task id for tasks and plans,
/// `<task>_<run>` for agents, and one fixed key for the single-row usage and settings topics.
fn row_key(topic: Topic, row: &Value) -> Option<String> {
    let text = |value: &Value| value.as_str().map(str::to_owned);
    match topic {
        Topic::Tasks => text(&row["task"]["id"]),
        Topic::Plans => text(&row["parent"]["id"]),
        Topic::Agents => Some(format!(
            "{}_{}",
            row["run"]["task"].as_str()?,
            row["run"]["id"].as_str()?
        )),
        Topic::Usage => Some("usage".into()),
        Topic::Settings => Some("settings".into()),
        _ => text(&row["id"]),
    }
}
#[derive(Default)]
struct PageBatch {
    epoch: Option<u64>,
    replacement: bool,
    rows: BTreeMap<String, Value>,
    left: BTreeSet<String>,
    bytes: usize,
}

pub(super) struct Core {
    next_id: u64,
    pending: BTreeMap<u64, Pending>,
    welcome: Option<Welcome>,
    events: VecDeque<Event>,
    outbox: VecDeque<Frame>,
    pages: BTreeMap<String, PageBatch>,
    deferred_opens: BTreeMap<String, (u64, Frame, SubOpen)>,
    adjacent_reset: Option<(Topic, Option<String>)>,
    reset_errors: BTreeMap<String, u8>,
}

impl Core {
    pub(super) fn new() -> Self {
        Self {
            next_id: 0,
            pending: BTreeMap::new(),
            welcome: None,
            events: VecDeque::new(),
            outbox: VecDeque::new(),
            pages: BTreeMap::new(),
            deferred_opens: BTreeMap::new(),
            adjacent_reset: None,
            reset_errors: BTreeMap::new(),
        }
    }

    fn clear_topic_errors(&mut self, key: &str) {
        self.reset_errors.remove(key);
        if self
            .adjacent_reset
            .as_ref()
            .is_some_and(|(topic, scope)| topic_key(*topic, scope.as_deref()) == key)
        {
            self.adjacent_reset = None;
        }
    }

    pub(super) fn hello(&mut self, caps: &[&str], now_ms: u64) -> Result<(), String> {
        let hello = Hello {
            proto: Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps: caps.iter().map(|cap| (*cap).into()).collect(),
            client: ClientInfo {
                kind: "app".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        };
        self.request(
            wire::HELLO,
            serde_json::to_value(hello).map_err(decode)?,
            now_ms,
        )
    }

    pub(super) fn request(
        &mut self,
        method: &'static str,
        params: Value,
        now_ms: u64,
    ) -> Result<(), String> {
        self.request_tagged(method, params, None, now_ms)
    }

    pub(super) fn request_tagged(
        &mut self,
        method: &'static str,
        params: Value,
        tag: Option<String>,
        now_ms: u64,
    ) -> Result<(), String> {
        self.queue_request(method, params, tag, now_ms, false)
    }

    fn queue_request(
        &mut self,
        method: &'static str,
        params: Value,
        tag: Option<String>,
        now_ms: u64,
        reset_reopen: bool,
    ) -> Result<(), String> {
        let payload = if self.pending.len() < 1024 && self.next_id < u64::MAX {
            request_payload(method, &params, self.next_id + 1)?
        } else {
            Vec::new()
        };
        let problem = if self.pending.len() >= 1024 {
            Some((ErrorCode::Busy, "daemon request limit exceeded".to_owned()))
        } else if payload.len() > wire::app::APP_CONTROL_MAX {
            Some((
                ErrorCode::TooLarge,
                "daemon CONTROL request exceeds 60 KiB".to_owned(),
            ))
        } else if self.next_id == u64::MAX {
            Some((
                ErrorCode::Internal,
                "daemon request ids exhausted".to_owned(),
            ))
        } else {
            None
        };
        if let Some((code, message)) = problem {
            if matches!(method, wire::HELLO | wire::PING) {
                return Err(message);
            }
            self.events.push_back(Event::Reply {
                method,
                tag,
                params,
                outcome: Err((code, message)),
                error_data: None,
            });
            return Ok(());
        }
        self.next_id += 1;
        let mut opening = None;
        let closing = if method == wire::SUB_CLOSE {
            let close: SubClose = serde_json::from_value(params.clone()).map_err(decode)?;
            let key = topic_key(close.topic, close.scope.as_deref());
            if let Some((id, _, _)) = self.deferred_opens.remove(&key) {
                self.pending.remove(&id);
            }
            Some(key)
        } else {
            None
        };
        if method == wire::SUB_OPEN {
            let open: SubOpen = serde_json::from_value(params.clone()).map_err(decode)?;
            opening = Some((topic_key(open.topic, open.scope.as_deref()), open));
        }
        let frame = Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload,
        };
        let mut deadline = now_ms.saturating_add(request_deadline(method, &params));
        if let Some((key, open)) = opening {
            if self
                .pending
                .values()
                .any(|pending| pending.closing.as_ref() == Some(&key))
            {
                // Only the latest intent survives, without extending its original deadline.
                if let Some((id, _, _)) =
                    self.deferred_opens.insert(key, (self.next_id, frame, open))
                    && let Some(previous) = self.pending.remove(&id)
                {
                    deadline = previous.deadline.unwrap_or(deadline);
                }
            } else {
                self.prepare_open(&open, reset_reopen);
                self.outbox.push_back(frame);
            }
        } else {
            self.outbox.push_back(frame);
        }
        self.pending.insert(
            self.next_id,
            Pending {
                method,
                tag,
                params,
                deadline: Some(deadline),
                closing,
            },
        );
        Ok(())
    }

    fn prepare_open(&mut self, open: &SubOpen, reset_reopen: bool) {
        let key = topic_key(open.topic, open.scope.as_deref());
        if !reset_reopen {
            self.clear_topic_errors(&key);
        }
        if !matches!(open.topic, Topic::Health | Topic::Other) {
            self.pages.insert(
                key,
                PageBatch {
                    replacement: true,
                    ..PageBatch::default()
                },
            );
        }
    }

    #[cfg(test)]
    pub(super) fn open(&mut self, topic: Topic, now_ms: u64) -> Result<(), String> {
        self.request(
            wire::SUB_OPEN,
            serde_json::to_value(SubOpen {
                topic,
                since: None,
                scope: None,
            })
            .map_err(decode)?,
            now_ms,
        )
    }

    pub(super) fn take_welcome(&mut self) -> Option<Welcome> {
        self.welcome.take()
    }

    pub(super) fn take_outbox(&mut self) -> Option<Frame> {
        self.outbox.pop_front()
    }

    pub(super) fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Heartbeats must not race a daemon operation waiting on its reader worker.
    pub(super) fn has_pending(&self) -> bool {
        self.pending
            .values()
            .any(|pending| pending.deadline.is_some())
    }

    /// Resolve operation deadlines as replies; only hello/ping expiry kills the link.
    pub(super) fn expired(&mut self, now_ms: u64) -> Option<&'static str> {
        loop {
            let id = self
                .pending
                .iter()
                .find(|(_, pending)| pending.deadline.is_some_and(|deadline| deadline <= now_ms))
                .map(|(id, _)| *id)?;
            let pending = self.pending.get_mut(&id)?;
            if matches!(pending.method, wire::HELLO | wire::PING) {
                return Some(pending.method);
            }
            self.events.push_back(Event::Reply {
                method: pending.method,
                tag: pending.tag.take(),
                params: pending.params.take(),
                outcome: Err((
                    ErrorCode::Unavailable,
                    "daemon request deadline expired".into(),
                )),
                error_data: Some(serde_json::json!({ "code": DEADLINE_EXPIRED })),
            });
            if pending.closing.is_some() {
                // A timed-out close still owns the ordered retirement barrier.
                pending.deadline = None;
            } else {
                self.pending.remove(&id);
                self.deferred_opens
                    .retain(|_, (opening, _, _)| *opening != id);
            }
        }
    }

    pub(super) fn receive(&mut self, frame: Frame, now_ms: u64) -> Result<(), String> {
        if frame.kind != FrameKind::Control || frame.stream != 0 {
            return Err("daemon sent a non-control frame".into());
        }
        if frame.payload.len() > corral_protocol::app::APP_CONTROL_MAX {
            return Err("daemon CONTROL reply exceeds 60 KiB".into());
        }
        // AppTopics::flush emits reset then null-id Unavailable synchronously on one Conn.
        // Taking on every frame prevents attribution across intervening scopes/health/replies.
        let adjacent_reset = self.adjacent_reset.take();
        let value: Value = serde_json::from_slice(&frame.payload).map_err(decode)?;
        if value.get("method").is_some() {
            if value.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC) {
                return Err("invalid daemon notification version".into());
            }
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| "invalid daemon notification method".to_owned())?;
            if !matches!(method, wire::SUB_DELTA | wire::SUB_RESET) {
                self.events.push_back(Event::Notice {
                    method: method.into(),
                    params: value.get("params").cloned().unwrap_or(Value::Null),
                });
                return Ok(());
            }
            let notification: Notification = serde_json::from_value(value).map_err(decode)?;
            self.notification(notification, now_ms, frame.payload.len())?;
        } else {
            let response: Response = serde_json::from_value(value).map_err(decode)?;
            if response.jsonrpc != JSONRPC {
                return Err("invalid daemon response version".into());
            }
            if response.id.is_null() {
                if let (Some((topic, scope)), ResponseOutcome::Error { error }) =
                    (adjacent_reset, response.outcome)
                    && error.code == ErrorCode::Unavailable
                {
                    let failures = self
                        .reset_errors
                        .entry(topic_key(topic, scope.as_deref()))
                        .or_default();
                    *failures = failures.saturating_add(1);
                    self.events.push_back(Event::TopicError {
                        topic,
                        scope,
                        code: error.code,
                        message: error.message,
                    });
                }
                return Ok(());
            }
            let Some(pending) = response.id.as_u64().and_then(|id| self.pending.remove(&id)) else {
                return Ok(());
            };
            let internal = matches!(
                pending.method,
                wire::HELLO | wire::SUB_OPEN | wire::SUB_ACK | wire::SUB_CLOSE
            );
            let result = match response.outcome {
                ResponseOutcome::Result { result } => result,
                ResponseOutcome::Error { error } => {
                    if pending.method == wire::SUB_ACK {
                        let mut message = format!(
                            "daemon {}: {} (topic={}, scope={}, cursor={})",
                            pending.method,
                            error.message,
                            pending.params["topic"],
                            pending.params["scope"],
                            pending.params["cursor"],
                        );
                        while message.len() > 1024 {
                            message.pop();
                        }
                        return Err(message);
                    }
                    if internal {
                        return Err(format!("daemon {}: {}", pending.method, error.message));
                    }
                    self.events.push_back(Event::Reply {
                        tag: pending.tag,
                        params: pending.params,
                        method: pending.method,
                        error_data: error.data,
                        outcome: Err((error.code, error.message)),
                    });
                    return Ok(());
                }
            };
            if let Some(key) = pending.closing.as_ref()
                && !self
                    .pending
                    .values()
                    .any(|request| request.closing.as_ref() == Some(key))
            {
                self.pages.remove(key);
                self.clear_topic_errors(key);
                if let Some((_, frame, open)) = self.deferred_opens.remove(key) {
                    self.prepare_open(&open, false);
                    self.outbox.push_back(frame);
                }
            }
            if pending.method == wire::HELLO {
                let welcome: Welcome = serde_json::from_value(result).map_err(decode)?;
                if welcome.proto.major != wire::PROTO_MAJOR {
                    return Err(format!(
                        "unsupported daemon protocol major {}",
                        welcome.proto.major
                    ));
                }
                self.welcome = Some(welcome);
            } else if !internal {
                self.events.push_back(Event::Reply {
                    tag: pending.tag,
                    params: pending.params,
                    method: pending.method,
                    error_data: None,
                    outcome: Ok(result),
                });
            }
        }
        Ok(())
    }

    fn notification(
        &mut self,
        notification: Notification,
        now_ms: u64,
        page_bytes: usize,
    ) -> Result<(), String> {
        let topic_value = notification
            .params
            .get("topic")
            .cloned()
            .unwrap_or(Value::Null);
        let topic: Topic = serde_json::from_value(topic_value).map_err(decode)?;
        if topic == Topic::Other {
            return Ok(());
        }
        let key = topic_key(topic, notification.params["scope"].as_str());
        if self
            .pending
            .values()
            .any(|pending| pending.closing.as_ref() == Some(&key))
        {
            // The producer may have sent this page before receiving our close.
            // It belongs to the retired subscription: neither publish nor ACK it.
            return Ok(());
        }
        match notification.method.as_str() {
            wire::SUB_DELTA => {
                self.reset_errors
                    .remove(&topic_key(topic, notification.params["scope"].as_str()));
                if topic != Topic::Health {
                    return self.rows_page(notification.params, now_ms, page_bytes);
                }
                let delta: SubDelta =
                    serde_json::from_value(notification.params).map_err(decode)?;
                let SubDelta::Health { rows, cursor, .. } = delta else {
                    return Ok(());
                };
                if let Some(health) = rows.into_iter().last() {
                    self.events.push_back(Event::Health(health));
                }
                let topic = Topic::Health;
                self.request(
                    wire::SUB_ACK,
                    serde_json::to_value(SubAck {
                        topic,
                        cursor,
                        scope: None,
                    })
                    .map_err(decode)?,
                    now_ms,
                )?;
            }
            wire::SUB_RESET => {
                let reset: SubReset =
                    serde_json::from_value(notification.params).map_err(decode)?;
                self.adjacent_reset = Some((reset.topic, reset.scope.clone()));
                self.events.push_back(Event::Reset {
                    topic: reset.topic,
                    scope: reset.scope.clone(),
                    epoch: reset.epoch,
                });
                if self
                    .reset_errors
                    .get(&topic_key(reset.topic, reset.scope.as_deref()))
                    .is_some_and(|count| *count >= MAX_TOPIC_RESET_ERRORS)
                {
                    return Ok(());
                }
                if reset.topic != Topic::Health {
                    let key = topic_key(reset.topic, reset.scope.as_deref());
                    self.pages.insert(
                        key,
                        PageBatch {
                            epoch: Some(reset.epoch),
                            replacement: true,
                            ..PageBatch::default()
                        },
                    );
                }
                self.queue_request(
                    wire::SUB_OPEN,
                    serde_json::to_value(SubOpen {
                        topic: reset.topic,
                        since: None,
                        scope: reset.scope,
                    })
                    .map_err(decode)?,
                    None,
                    now_ms,
                    true,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn rows_page(
        &mut self,
        mut value: Value,
        now_ms: u64,
        page_bytes: usize,
    ) -> Result<(), String> {
        let topic: Topic = serde_json::from_value(value["topic"].clone()).map_err(decode)?;
        let scope = value["scope"].as_str().map(str::to_owned);
        let key = topic_key(topic, scope.as_deref());
        let epoch = value["epoch"].as_u64().ok_or("missing topic epoch")?;
        let batch = self.pages.entry(key).or_insert_with(|| PageBatch {
            replacement: true,
            ..PageBatch::default()
        });
        if batch.epoch.is_some_and(|current| current != epoch) {
            return Err("daemon topic epoch changed without sub.reset".into());
        }
        batch.epoch = Some(epoch);
        batch.bytes = batch.bytes.saturating_add(page_bytes);
        if matches!(topic, Topic::Session | Topic::Terminal) {
            if scope.is_none() {
                return Err("scoped session delta has no scope".into());
            }
            if topic == Topic::Terminal && value["more"].as_bool() != Some(false) {
                return Err("terminal delta is not a complete capture".into());
            }
            let cursor = serde_json::from_value(value["cursor"].clone()).map_err(decode)?;
            let replacement = batch.replacement;
            batch.replacement = false;
            batch.bytes = 0;
            let delta = serde_json::from_value(value).map_err(decode)?;
            self.events.push_back(Event::Rows { delta, replacement });
            return self.request(
                wire::SUB_ACK,
                serde_json::to_value(SubAck {
                    topic,
                    cursor,
                    scope,
                })
                .map_err(decode)?,
                now_ms,
            );
        }
        if let Some(rows) = value["rows"].as_array_mut() {
            for row in std::mem::take(rows) {
                let id = row_key(topic, &row).ok_or("topic row has no id")?;
                batch.left.remove(&id);
                batch.rows.insert(id, row);
            }
        }
        if let Some(left) = value["left"].as_array() {
            for id in left {
                let id = id.as_str().ok_or("invalid left row id")?.to_owned();
                batch.rows.remove(&id);
                batch.left.insert(id);
            }
        }
        if batch.rows.len() > 50_000 || batch.bytes > 32 * 1024 * 1024 {
            return Err("daemon topic snapshot exceeds client bounds".into());
        }
        let cursor = serde_json::from_value(value["cursor"].clone()).map_err(decode)?;
        if value["more"].as_bool() != Some(true) {
            let replacement = batch.replacement;
            value["rows"] = Value::Array(std::mem::take(&mut batch.rows).into_values().collect());
            value["left"] = Value::Array(
                std::mem::take(&mut batch.left)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            );
            batch.bytes = 0;
            batch.replacement = false;
            let delta: SubDelta = serde_json::from_value(value).map_err(decode)?;
            self.events.push_back(Event::Rows { delta, replacement });
        }
        self.request(
            wire::SUB_ACK,
            serde_json::to_value(SubAck {
                topic,
                cursor,
                scope,
            })
            .map_err(decode)?,
            now_ms,
        )
    }
}

pub(super) fn topic_key(topic: Topic, scope: Option<&str>) -> String {
    let name = serde_json::to_value(topic)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".into());
    scope.map_or(name.clone(), |scope| format!("{name}:{scope}"))
}

#[derive(serde::Serialize)]
struct ControlEnvelope<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: &'a Value,
}

fn request_payload(method: &str, params: &Value, id: u64) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&ControlEnvelope {
        jsonrpc: JSONRPC,
        id,
        method,
        params,
    })
    .map_err(decode)
}

/// Reserve the largest RPC id so admission remains valid as the connection ages.
pub(super) fn preflight_request(method: &str, params: &Value) -> Result<(), String> {
    struct Size(usize);
    impl std::io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut size = Size(0);
    serde_json::to_writer(
        &mut size,
        &ControlEnvelope {
            jsonrpc: JSONRPC,
            id: u64::MAX,
            method,
            params,
        },
    )
    .map_err(decode)?;
    if size.0 > wire::app::APP_CONTROL_MAX {
        return Err("daemon CONTROL request exceeds 60 KiB".into());
    }
    Ok(())
}

fn request_deadline(method: &str, params: &Value) -> u64 {
    match method {
        wire::app::DETAIL_GET | wire::app::CHANGES_PAGE | wire::app::SEEN_ADVANCE => {
            APP_READ_DEADLINE_MS
        }
        wire::device::PAIR_STATUS => params["wait_ms"]
            .as_u64()
            .unwrap_or(0)
            .min(APP_READ_WAIT_MS)
            .saturating_add(APP_REQUEST_MARGIN_MS),
        // A card answer, a host action and a build's removal wait on the host or on prune.
        wire::op::OP_SUBMIT
        | wire::session_host::SESSION_CARD_ANSWER
        | wire::session_host::SESSION_ACTION
        | wire::build::BUILD_REMOVE => APP_COMMAND_DEADLINE_MS,
        _ => APP_REQUEST_DEADLINE_MS,
    }
}

fn decode(error: serde_json::Error) -> String {
    format!("invalid daemon JSON: {error}")
}

#[cfg(test)]
#[path = "topic_error_tests.rs"]
mod topic_error_tests;

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod terminal_tests;
