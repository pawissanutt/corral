use corral_protocol::board::BoardRow;
use corral_protocol::frame::{Frame, FrameKind};
use corral_protocol::rpc::{ErrorCode, JSONRPC, Notification, Request, Response, ResponseOutcome};
use corral_protocol::subscription::{SubAck, SubDelta, SubOpen, SubReset, Topic};
use corral_protocol::{self as wire, ClientInfo, Health, Hello, Proto, Welcome};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const REQUEST_DEADLINE_MS: u64 = 5_000;

pub(super) enum Event {
    Health(Health),
    Board {
        epoch: u64,
        rows: Vec<BoardRow>,
        left: Vec<String>,
        replacement: bool,
    },
    Reply {
        method: &'static str,
        outcome: Result<Value, (ErrorCode, String)>,
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
    deadline: u64,
}

#[derive(Default)]
struct Batch {
    rows: BTreeMap<String, BoardRow>,
    left: BTreeSet<String>,
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
        self.outbox.push_back(Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload,
        });
        self.pending.insert(
            self.next_id,
            Pending {
                method,
                deadline: now_ms.saturating_add(REQUEST_DEADLINE_MS),
            },
        );
        Ok(())
    }

    pub(super) fn open(&mut self, topic: Topic, now_ms: u64) -> Result<(), String> {
        self.request(
            wire::SUB_OPEN,
            serde_json::to_value(SubOpen { topic, since: None }).map_err(decode)?,
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
            self.notification(notification, now_ms)?;
        } else {
            let response: Response = serde_json::from_value(value).map_err(decode)?;
            if response.jsonrpc != JSONRPC {
                return Err("invalid daemon response version".into());
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
                    if internal {
                        return Err(format!("daemon {}: {}", pending.method, error.message));
                    }
                    self.events.push_back(Event::Reply {
                        method: pending.method,
                        outcome: Err((error.code, error.message)),
                    });
                    return Ok(());
                }
            };
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
                    method: pending.method,
                    outcome: Ok(result),
                });
            }
        }
        Ok(())
    }

    fn notification(&mut self, notification: Notification, now_ms: u64) -> Result<(), String> {
        if !matches!(
            notification.params.get("topic").and_then(Value::as_str),
            Some("board" | "health")
        ) {
            return Ok(());
        }
        match notification.method.as_str() {
            wire::SUB_DELTA => {
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
                    SubDelta::Classic { .. } | SubDelta::Other => return Ok(()),
                };
                self.request(
                    wire::SUB_ACK,
                    serde_json::to_value(SubAck { topic, cursor }).map_err(decode)?,
                    now_ms,
                )?;
            }
            wire::SUB_RESET => {
                let reset: SubReset =
                    serde_json::from_value(notification.params).map_err(decode)?;
                match reset.topic {
                    Topic::Board => {
                        self.batch = Batch::default();
                        self.epoch = Some(reset.epoch);
                        self.replacement = true;
                        self.open(Topic::Board, now_ms)?;
                    }
                    Topic::Health => self.open(Topic::Health, now_ms)?,
                    Topic::Classic | Topic::Other => {}
                }
            }
            _ => {}
        }
        Ok(())
    }
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
}
