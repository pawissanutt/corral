use corral_protocol::board::BoardRow;
use corral_protocol::frame::{Frame, FrameKind};
use corral_protocol::rpc::{ErrorCode, JSONRPC, Notification, Request, Response, ResponseOutcome};
use corral_protocol::subscription::{SubAck, SubClose, SubDelta, SubOpen, SubReset, Topic};
use corral_protocol::{self as wire, ClientInfo, Health, Hello, Proto, Welcome};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const REQUEST_DEADLINE_MS: u64 = 5_000;

const MAX_TOPIC_RESET_ERRORS: u8 = 3;

pub(super) enum Event {
    Health(Health),
    Board {
        epoch: u64,
        rows: Vec<BoardRow>,
        left: Vec<String>,
        replacement: bool,
    },
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
    Live {
        scope: String,
        live: corral_protocol::subscription::LiveText,
    },
    Notice {
        method: String,
        params: Value,
    },
    Idle,
    Closed(String),
}

struct Pending {
    method: &'static str,
    tag: Option<String>,
    params: Value,
    deadline: u64,
    closing: Option<String>,
}

#[derive(Default)]
struct Batch {
    rows: BTreeMap<String, BoardRow>,
    left: BTreeSet<String>,
    bytes: usize,
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
    epoch: Option<u64>,
    replacement: bool,
    batch: Batch,
    outbox: VecDeque<Frame>,
    pages: BTreeMap<String, PageBatch>,
    deferred_opens: BTreeMap<String, (u64, Frame)>,
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
            epoch: None,
            replacement: true,
            batch: Batch::default(),
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
        if self.pending.len() >= 1024 {
            return Err("daemon request limit exceeded".into());
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "daemon request ids exhausted".to_owned())?;
        let request = Request {
            jsonrpc: JSONRPC.into(),
            id: self.next_id.into(),
            method: method.into(),
            params,
        };
        let payload = serde_json::to_vec(&request).map_err(decode)?;
        if payload.len() > corral_protocol::app::APP_CONTROL_MAX {
            return Err("daemon CONTROL request exceeds 60 KiB".into());
        }
        let mut opening = None;
        let closing = if method == wire::SUB_CLOSE {
            let close: SubClose = serde_json::from_value(request.params.clone()).map_err(decode)?;
            let key = topic_key(
                close.topic,
                close
                    .scope
                    .as_deref()
                    .filter(|_| matches!(close.topic, Topic::Thread | Topic::Session)),
            );
            self.clear_topic_errors(&key);
            if let Some((id, _)) = self.deferred_opens.remove(&key) {
                self.pending.remove(&id);
            }
            Some(key)
        } else {
            None
        };
        if method == wire::SUB_OPEN {
            let open: SubOpen = serde_json::from_value(request.params.clone()).map_err(decode)?;
            let key = topic_key(
                open.topic,
                open.scope
                    .as_deref()
                    .filter(|_| matches!(open.topic, Topic::Thread | Topic::Session)),
            );
            if !reset_reopen {
                self.clear_topic_errors(&key);
            }
            opening = Some(key);
            match open.topic {
                Topic::Board => {
                    self.batch = Batch::default();
                    self.epoch = None;
                    self.replacement = true;
                }
                Topic::Health | Topic::Classic | Topic::Other => {}
                topic => {
                    self.pages.insert(
                        topic_key(topic, open.scope.as_deref()),
                        PageBatch {
                            replacement: true,
                            ..PageBatch::default()
                        },
                    );
                }
            }
        }
        let frame = Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload,
        };
        if let Some(key) = opening.filter(|key| {
            self.pending
                .values()
                .any(|pending| pending.closing.as_ref() == Some(key))
        }) {
            // The close reply is an ordered barrier after every retired notification.
            // Keep only the latest reopen intent for this identity, within its deadline.
            if let Some((id, _)) = self.deferred_opens.insert(key, (self.next_id, frame)) {
                self.pending.remove(&id);
            }
        } else {
            self.outbox.push_back(frame);
        }
        self.pending.insert(
            self.next_id,
            Pending {
                method,
                tag,
                params: request.params,
                deadline: now_ms.saturating_add(REQUEST_DEADLINE_MS),
                closing,
            },
        );
        Ok(())
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

    pub(super) fn expired(&self, now_ms: u64) -> Option<&'static str> {
        self.pending
            .values()
            .find(|request| request.deadline <= now_ms)
            .map(|request| request.method)
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
                && let Some((_, frame)) = self.deferred_opens.remove(key)
            {
                self.outbox.push_back(frame);
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
        if matches!(topic, Topic::Other | Topic::Classic) {
            return Ok(());
        }
        if self.pending.values().any(|pending| {
            pending.closing.is_some()
                && pending.params.get("topic") == notification.params.get("topic")
                && (!matches!(topic, Topic::Thread | Topic::Session)
                    || pending.params["scope"].as_str() == notification.params["scope"].as_str())
        }) {
            // The producer may have sent this page before receiving our close.
            // It belongs to the retired subscription: neither publish nor ACK it.
            return Ok(());
        }
        match notification.method.as_str() {
            wire::SUB_DELTA => {
                self.reset_errors
                    .remove(&topic_key(topic, notification.params["scope"].as_str()));
                if !matches!(topic, Topic::Board | Topic::Health) {
                    return self.rows_page(notification.params, now_ms, page_bytes);
                }
                let delta: SubDelta =
                    serde_json::from_value(notification.params).map_err(decode)?;
                let (topic, cursor) = match delta {
                    SubDelta::Board { delta } => {
                        if self.epoch.is_some_and(|epoch| epoch != delta.epoch) {
                            return Err("daemon board epoch changed without sub.reset".into());
                        }
                        self.epoch = Some(delta.epoch);
                        for row in delta.rows {
                            self.batch.left.remove(&row.id);
                            self.batch.rows.insert(row.id.clone(), row);
                        }
                        for id in delta.left {
                            self.batch.rows.remove(&id);
                            self.batch.left.insert(id);
                        }
                        self.batch.bytes = self.batch.bytes.saturating_add(page_bytes);
                        if self.batch.rows.len() > 50_000 || self.batch.bytes > 32 * 1024 * 1024 {
                            return Err("daemon board snapshot exceeds client bounds".into());
                        }
                        if !delta.more {
                            let batch = std::mem::take(&mut self.batch);
                            self.events.push_back(Event::Board {
                                epoch: delta.epoch,
                                rows: batch.rows.into_values().collect(),
                                left: batch.left.into_iter().collect(),
                                replacement: self.replacement,
                            });
                            self.replacement = false;
                        }
                        (Topic::Board, delta.cursor)
                    }
                    SubDelta::Health { rows, cursor, .. } => {
                        if let Some(health) = rows.into_iter().last() {
                            self.events.push_back(Event::Health(health));
                        }
                        (Topic::Health, cursor)
                    }
                    _ => return Ok(()),
                };
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
                match reset.topic {
                    Topic::Board => {
                        self.batch = Batch::default();
                        self.epoch = Some(reset.epoch);
                        self.replacement = true;
                    }
                    Topic::Health => {}
                    _ => {
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
        if topic == Topic::Thread {
            if let Some(live) = value
                .get_mut("live")
                .map(Value::take)
                .filter(|live| !live.is_null())
            {
                self.events.push_back(Event::Live {
                    scope: scope.clone().ok_or("thread delta has no scope")?,
                    live: serde_json::from_value(live).map_err(decode)?,
                });
                let empty = |field: &str| value[field].as_array().is_none_or(Vec::is_empty);
                if empty("rows") && empty("left") {
                    let cursor = serde_json::from_value(value["cursor"].clone()).map_err(decode)?;
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
            }
        }
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
        if topic == Topic::Session {
            if scope.is_none() {
                return Err("session delta has no scope".into());
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
                let id = row["id"].as_str().ok_or("topic row has no id")?.to_owned();
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

fn decode(error: serde_json::Error) -> String {
    format!("invalid daemon JSON: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(value: Value) -> Frame {
        Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&value).unwrap(),
        }
    }

    #[test]
    fn device_errors_are_replies_but_subscription_errors_close_the_session() {
        let mut core = Core::new();
        core.request(
            corral_protocol::device::DEVICE_ATTACH,
            json!({"device":"dev-test"}),
            10,
        )
        .unwrap();
        core.receive(
            frame(json!({
                "jsonrpc":"2.0", "id":1,
                "error":{"code":"revoked", "message":"device_revoked"}
            })),
            11,
        )
        .unwrap();
        assert!(matches!(
            core.next_event(),
            Some(Event::Reply {
                method: corral_protocol::device::DEVICE_ATTACH,
                outcome: Err((ErrorCode::Revoked, _)),
                ..
            })
        ));
        core.open(Topic::Board, 12).unwrap();
        assert!(
            core.receive(
                frame(json!({
                    "jsonrpc":"2.0", "id":2,
                    "error":{"code":"refused", "message":"topic_refused"}
                })),
                13
            )
            .is_err()
        );
    }

    #[test]
    fn notices_survive_and_requests_expire_at_the_supplied_deadline() {
        let mut core = Core::new();
        core.request(wire::PING, json!({}), 100).unwrap();
        assert_eq!(core.expired(5_099), None);
        assert_eq!(core.expired(5_100), Some(wire::PING));
        core.receive(
            frame(json!({
                "jsonrpc":"2.0", "method":"bye", "params":{"reason":"revoked"}
            })),
            101,
        )
        .unwrap();
        assert!(
            matches!(core.next_event(), Some(Event::Notice { method, params })
            if method == "bye" && params["reason"] == "revoked")
        );
        core.receive(frame(json!({"jsonrpc":"2.0", "id":1, "result":{}})), 102)
            .unwrap();
        assert_eq!(core.expired(5_100), None);
    }

    #[test]
    fn live_frames_never_finish_or_replace_a_scoped_history_snapshot() {
        let mut core = Core::new();
        for (more, live) in [
            (true, Value::Null),
            (
                false,
                json!({
                    "entry":"reply-1","turn":1,"offset":0,"text":"answer","done":false
                }),
            ),
        ] {
            core.receive(
                frame(json!({"jsonrpc":"2.0","method":"sub.delta","params":{
                    "topic":"thread","scope":"quick","epoch":1,"rows":[],"left":[],
                    "cursor":{"rev":1,"after":null},"more":more,"live":live
                }})),
                0,
            )
            .unwrap();
        }
        assert!(matches!(core.next_event(), Some(Event::Live { scope, .. }) if scope == "quick"));
        assert!(core.next_event().is_none());
        core.receive(
            frame(json!({"jsonrpc":"2.0","method":"sub.delta","params":{
                "topic":"thread","scope":"quick","epoch":1,"rows":[],"left":[],
                "cursor":{"rev":1,"after":null},"more":false,"live":null
            }})),
            1,
        )
        .unwrap();
        assert!(matches!(
            core.next_event(),
            Some(Event::Rows {
                replacement: true,
                ..
            })
        ));
    }
}

#[cfg(test)]
#[path = "topic_error_tests.rs"]
mod topic_error_tests;
