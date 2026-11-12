use super::Event;
use crate::client::transport;
use corral_protocol::board::BoardRow;
use corral_protocol::frame::{Frame, FrameKind, read_frame_until, write_frame_until};
use corral_protocol::rpc::{JSONRPC, Notification, Request, Response, ResponseOutcome};
use corral_protocol::subscription::{SubAck, SubDelta, SubOpen, SubReset, Topic};
use corral_protocol::{self as wire, ClientInfo, Hello, Proto, Welcome};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

struct Pending {
    method: &'static str,
    deadline: Instant,
}

#[derive(Default)]
struct Batch {
    rows: BTreeMap<String, BoardRow>,
    left: BTreeSet<String>,
}

pub(crate) struct Session {
    stream: UnixStream,
    next_id: u64,
    pending: BTreeMap<u64, Pending>,
    welcome: Option<Welcome>,
    events: VecDeque<Event>,
    epoch: Option<u64>,
    replacement: bool,
    batch: Batch,
}

impl Session {
    pub(crate) fn connect(socket: &Path) -> Result<(Self, Welcome), String> {
        let deadline = Instant::now() + IO_TIMEOUT;
        let stream = transport::connect(socket, deadline)?;
        let mut session = Self {
            stream,
            next_id: 0,
            pending: BTreeMap::new(),
            welcome: None,
            events: VecDeque::new(),
            epoch: None,
            replacement: true,
            batch: Batch::default(),
        };
        let hello = Hello {
            proto: Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps: vec![wire::BOARD_V1.into()],
            client: ClientInfo {
                kind: "app".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        };
        session.send(wire::HELLO, serde_json::to_value(hello).map_err(decode)?)?;
        loop {
            if let Some(welcome) = session.welcome.take() {
                session.open(Topic::Health)?;
                session.open(Topic::Board)?;
                return Ok((session, welcome));
            }
            if !session.receive(deadline)? {
                return Err("daemon hello deadline expired".into());
            }
        }
    }

    pub(crate) fn next(&mut self) -> Event {
        match self.next_event() {
            Ok(event) => event,
            Err(reason) => Event::Closed(one_line(reason)),
        }
    }

    fn next_event(&mut self) -> Result<Event, String> {
        let deadline = Instant::now() + IDLE_TIMEOUT;
        loop {
            let now = Instant::now();
            if let Some(pending) = self
                .pending
                .values()
                .find(|request| request.deadline <= now)
            {
                return Err(format!("daemon {} deadline expired", pending.method));
            }
            if let Some(event) = self.events.pop_front() {
                return Ok(event);
            }
            if now >= deadline {
                return Ok(Event::Idle);
            }
            let until = self
                .pending
                .values()
                .map(|request| request.deadline)
                .min()
                .map_or(deadline, |request| deadline.min(request));
            // A quiet socket is not a framing error; a partially read frame is.
            if !self.receive(until)? && until == deadline {
                return Ok(Event::Idle);
            }
        }
    }

    fn send(&mut self, method: &'static str, params: Value) -> Result<(), String> {
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
        let deadline = Instant::now() + IO_TIMEOUT;
        write_frame_until(
            &mut self.stream,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload,
            },
            deadline,
        )
        .map_err(|error| format!("sending daemon {method}: {error}"))?;
        self.pending
            .insert(self.next_id, Pending { method, deadline });
        Ok(())
    }

    fn open(&mut self, topic: Topic) -> Result<(), String> {
        self.send(
            wire::SUB_OPEN,
            serde_json::to_value(SubOpen { topic, since: None }).map_err(decode)?,
        )
    }

    fn receive(&mut self, deadline: Instant) -> Result<bool, String> {
        if !transport::readable(&self.stream, deadline)? {
            return Ok(false);
        }
        let frame = read_frame_until(&mut self.stream, deadline)
            .map_err(|error| format!("reading daemon frame: {error}"))?
            .ok_or_else(|| "daemon closed the socket".to_owned())?;
        if frame.kind != FrameKind::Control || frame.stream != 0 {
            return Err("daemon sent a non-control frame".into());
        }
        let value: Value = serde_json::from_slice(&frame.payload).map_err(decode)?;
        if value.get("method").is_some() {
            if value.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC) {
                return Err("invalid daemon notification version".into());
            }
            if !matches!(
                value.get("method").and_then(Value::as_str),
                Some(wire::SUB_DELTA | wire::SUB_RESET)
            ) {
                return Ok(true);
            }
            let notification: Notification = serde_json::from_value(value).map_err(decode)?;
            self.notification(notification)?;
        } else {
            let response: Response = serde_json::from_value(value).map_err(decode)?;
            if response.jsonrpc != JSONRPC {
                return Err("invalid daemon response version".into());
            }
            let Some(pending) = response.id.as_u64().and_then(|id| self.pending.remove(&id)) else {
                return Ok(true);
            };
            let result = match response.outcome {
                ResponseOutcome::Result { result } => result,
                ResponseOutcome::Error { error } => {
                    return Err(format!("daemon {}: {}", pending.method, error.message));
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
            }
        }
        Ok(true)
    }

    fn notification(&mut self, notification: Notification) -> Result<(), String> {
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
                    SubDelta::Other => return Ok(()),
                };
                self.send(
                    wire::SUB_ACK,
                    serde_json::to_value(SubAck { topic, cursor }).map_err(decode)?,
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
                        self.open(Topic::Board)?;
                    }
                    Topic::Health => self.open(Topic::Health)?,
                    Topic::Other => {}
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
fn one_line(text: String) -> String {
    if text.contains(['\r', '\n']) {
        text.replace(['\r', '\n'], " ")
    } else {
        text
    }
}
