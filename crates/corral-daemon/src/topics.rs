//! Acknowledged observations, one page in flight per subscription.
mod app;
pub(crate) use app::TerminalCache;

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind},
    rpc,
    subscription::{SubAck, SubClose, SubOpen, SubReset, Topic},
};
use serde::Serialize;

use crate::{conn::Conn, state::Shared};

pub(crate) struct Subscriptions<'a> {
    shared: &'a Shared,
    rows: Vec<app::Subscriber>,
    terminals: app::TerminalTopics,
}

impl<'a> Subscriptions<'a> {
    pub fn new(shared: &'a Shared) -> Self {
        Self {
            shared,
            rows: Vec::new(),
            terminals: app::TerminalTopics::default(),
        }
    }

    pub fn open(&mut self, open: SubOpen) -> Result<()> {
        if open.topic == Topic::Terminal {
            return self.terminals.open(self.shared, open);
        }
        ensure!(open.scope.is_none(), "topic does not take a scope");
        ensure!(
            matches!(
                open.topic,
                Topic::Health
                    | Topic::Tasks
                    | Topic::Plans
                    | Topic::Projects
                    | Topic::Agents
                    | Topic::Usage
                    | Topic::Settings
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
        } else if close.scope.is_none() {
            self.rows.retain(|sub| sub.topic != close.topic);
        }
    }

    pub fn flush(&mut self, conn: &mut dyn Conn) -> Result<()> {
        let mut i = 0;
        while i < self.rows.len() {
            let sub = &mut self.rows[i];
            let result = sub.next(self.shared);
            let close = match result {
                Ok(Some((frame, close))) => {
                    conn.send_frame(&frame).context("pushing topic frame")?;
                    close
                }
                Ok(None) => false,
                Err(error) => {
                    conn.send_frame(&reset(sub.topic, self.shared.epoch(), None)?)?;
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
        self.terminals.flush(self.shared, conn)
    }
}

fn reset(topic: Topic, epoch: u64, scope: Option<String>) -> Result<Frame> {
    notification(
        wire::SUB_RESET,
        SubReset {
            topic,
            epoch,
            scope,
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
