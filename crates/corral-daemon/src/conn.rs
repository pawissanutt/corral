//! Transport-neutral control replies and per-connection worker accounting.
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_protocol::{
    frame::{Frame, FrameKind, write_frame_until},
    rpc,
};
use serde::Serialize;
use serde_json::Value;

use crate::state::Shared;

pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) trait Conn {
    fn send_frame(&mut self, frame: &Frame) -> Result<()>;
}

impl Conn for UnixStream {
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        write_frame_until(self, frame, Instant::now() + IO_TIMEOUT).context("writing daemon frame")
    }
}

fn send(conn: &mut dyn Conn, value: &impl Serialize) -> Result<()> {
    let payload = serde_json::to_vec(value).context("encoding daemon response")?;
    conn.send_frame(&Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload,
    })
}

pub(crate) fn reply(conn: &mut dyn Conn, id: Value, result: impl Serialize) -> Result<()> {
    send(
        conn,
        &rpc::Response {
            jsonrpc: rpc::JSONRPC.into(),
            id,
            outcome: rpc::ResponseOutcome::Result { result },
        },
    )
}

pub(crate) fn error(
    conn: &mut dyn Conn,
    id: Value,
    code: rpc::ErrorCode,
    message: &str,
) -> Result<()> {
    let response: rpc::Response<Value> = rpc::Response {
        jsonrpc: rpc::JSONRPC.into(),
        id,
        outcome: rpc::ResponseOutcome::Error {
            error: rpc::Error {
                code,
                message: message.into(),
                data: None,
            },
        },
    };
    send(conn, &response)
}

pub(crate) fn notify(conn: &mut dyn Conn, method: &str, params: impl Serialize) -> Result<()> {
    send(
        conn,
        &rpc::Notification {
            jsonrpc: rpc::JSONRPC.into(),
            method: method.into(),
            params,
        },
    )
}

pub(crate) struct WorkerPermit(pub(crate) Arc<Shared>);

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.workers -= 1;
        self.0.changed.notify_all();
    }
}
