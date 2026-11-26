//! Acknowledged observations, one page in flight per subscription.
mod app;
pub(crate) use app::TerminalCache;
pub(crate) use app::{SessionLive, join_session_live, start_session_live};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind},
    rpc,
    subscription::{SubAck, SubClose, SubOpen, SubReset, Topic},
};
use serde::Serialize;

use crate::{conn::Conn, state::Shared, views::SystemFeatures};

pub(crate) struct Subscriptions<'a> {
    shared: &'a Shared,
    rows: Vec<app::Subscriber>,
    terminals: app::TerminalTopics,
    sessions: app::SessionTopics,
    // Deferred read replies use this connection's regular, single-writer flush.
    pub(crate) model_discovery: crate::config::pi_hosted_tool_models::Pending,
    session_fleet: bool,
    session_state: bool,
    build_rows: bool,
    system: bool,
    host: bool,
    system_features: SystemFeatures,
}

impl<'a> Subscriptions<'a> {
    pub fn new(shared: &'a Shared) -> Self {
        Self {
            shared,
            rows: Vec::new(),
            terminals: app::TerminalTopics::default(),
            sessions: app::SessionTopics::default(),
            model_discovery: Default::default(),
            session_fleet: false,
            session_state: false,
            build_rows: false,
            system: false,
            host: false,
            system_features: SystemFeatures::default(),
        }
    }

    pub(crate) fn set_capabilities(&mut self, caps: &[String]) {
        self.session_fleet = caps.iter().any(|cap| cap == wire::app::SESSION_FLEET_V1);
        self.session_state = caps.iter().any(|cap| cap == wire::app::SESSION_STATE_V1);
        self.build_rows = caps.iter().any(|cap| cap == wire::app::BUILD_V1);
        self.system = caps.iter().any(|cap| cap == wire::app::SYSTEM_V1);
        self.host = caps.iter().any(|cap| cap == wire::host::HOST_V1);
        self.system_features = SystemFeatures {
            runtime: self.system && caps.iter().any(|cap| cap == wire::app::SYSTEM_RUNTIME_V1),
            entries: self.system && caps.iter().any(|cap| cap == wire::app::SYSTEM_ENTRIES_V1),
            lenses: self.system && caps.iter().any(|cap| cap == wire::app::SYSTEM_LENSES_V1),
            data: self.system && caps.iter().any(|cap| cap == wire::app::SYSTEM_DATA_V1),
        };
        if !self.system {
            self.rows.retain(|sub| sub.topic != Topic::System);
        }
        if !self.host {
            self.rows.retain(|sub| sub.topic != Topic::Host);
        }
    }

    pub(crate) fn has_system(&self) -> bool {
        self.system
    }

    pub(crate) fn system_features(&self) -> SystemFeatures {
        self.system_features
    }

    pub fn open(&mut self, open: SubOpen) -> Result<()> {
        if open.topic == Topic::Terminal {
            return self.terminals.open(self.shared, open);
        }
        if open.topic == Topic::Session {
            return self.sessions.open(self.shared, open);
        }
        ensure!(open.scope.is_none(), "topic does not take a scope");
        ensure!(
            open.topic != Topic::System || self.system,
            "system.v1 required"
        );
        ensure!(open.topic != Topic::Host || self.host, "host.v1 required");
        ensure!(
            matches!(
                open.topic,
                Topic::Health
                    | Topic::Tasks
                    | Topic::Plans
                    | Topic::Projects
                    | Topic::System
                    | Topic::Agents
                    | Topic::Assistant
                    | Topic::Usage
                    | Topic::Settings
                    | Topic::Host
            ),
            "unsupported topic"
        );
        self.rows.retain(|sub| sub.topic != open.topic);
        self.rows
            .push(app::Subscriber::new(self.shared.epoch(), open));
        Ok(())
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        if ack.topic == Topic::Terminal {
            return self.terminals.ack(ack);
        }
        if ack.topic == Topic::Session {
            return self.sessions.ack(ack);
        }
        ensure!(ack.scope.is_none(), "topic does not take a scope");
        self.rows
            .iter_mut()
            .find(|sub| sub.topic == ack.topic)
            .context("topic is not open")?
            .ack(ack.cursor)
    }

    pub fn close(&mut self, close: SubClose) {
        if close.topic == Topic::Terminal {
            self.terminals.close(close.scope);
        } else if close.topic == Topic::Session {
            self.sessions.close(close.scope);
        } else if close.scope.is_none() {
            self.rows.retain(|sub| sub.topic != close.topic);
        }
    }

    /// Ends this connection's wait for client input when a watched session has a page.
    pub(crate) fn waker(&self) -> Option<std::sync::Arc<crate::wake::Waker>> {
        self.sessions.waker()
    }

    pub fn flush(&mut self, conn: &mut dyn Conn) -> Result<()> {
        // Before the buffers are read: what is published from here on wakes the next wait.
        if let Some(waker) = self.sessions.waker() {
            waker.clear();
        }
        self.model_discovery.flush(conn)?;
        let mut i = 0;
        while i < self.rows.len() {
            let sub = &mut self.rows[i];
            let result = sub.next(self.shared, self.build_rows);
            let close = match result {
                Ok(Some((frame, close))) => {
                    conn.send_frame(&frame).context("pushing topic frame")?;
                    close
                }
                Ok(None) => false,
                Err(error) => {
                    conn.send_frame(&reset(sub.topic, self.shared.epoch())?)?;
                    let mut message = format!("{error:#}");
                    message.truncate(message.floor_char_boundary(1024));
                    crate::conn::error(
                        conn,
                        serde_json::Value::Null,
                        rpc::ErrorCode::Unavailable,
                        &message,
                    )?;
                    true
                }
            };
            if close {
                self.rows.remove(i);
            } else {
                i += 1;
            }
        }
        self.terminals.flush(self.shared, conn)?;
        self.sessions
            .flush(self.shared, conn, self.session_fleet, self.session_state)
    }
}

fn reset(topic: Topic, epoch: u64) -> Result<Frame> {
    notification(
        wire::SUB_RESET,
        SubReset {
            topic,
            epoch,
            scope: None,
        },
    )
}

pub(crate) fn notification(method: &str, params: impl Serialize) -> Result<Frame> {
    let payload = serde_json::to_vec(&rpc::Notification {
        jsonrpc: rpc::JSONRPC.into(),
        method: method.into(),
        params,
    })
    .context("encoding subscription notification")?;
    Ok(Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload,
    })
}
