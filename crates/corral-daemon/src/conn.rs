//! Transport-neutral control replies and per-connection worker accounting.
use std::{
    os::unix::net::UnixStream,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use corral_protocol::{
    frame::{CONTROL_MAX, Frame, FrameKind, write_frame_until},
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

fn send(conn: &mut dyn Conn, payload: Vec<u8>) -> Result<()> {
    conn.send_frame(&Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload,
    })
}

pub(crate) fn reply<T: Serialize>(conn: &mut dyn Conn, id: Value, result: T) -> Result<()> {
    send_reply(conn, id, rpc::ResponseOutcome::Result { result })
}

pub(crate) fn rpc_error(conn: &mut dyn Conn, id: Value, mut error: rpc::Error) -> Result<()> {
    if error.code == rpc::ErrorCode::Refused && error.data.is_none() {
        error = rpc::Error::refused(corral_types::Refusal {
            reason: error.message,
            next: Vec::new(),
        })
        .context("encode refusal details")?;
    }
    send_reply::<()>(conn, id, rpc::ResponseOutcome::Error { error })
}

pub(crate) fn error(
    conn: &mut dyn Conn,
    id: Value,
    code: rpc::ErrorCode,
    message: &str,
) -> Result<()> {
    rpc_error(
        conn,
        id,
        rpc::Error {
            code,
            message: message.into(),
            data: None,
        },
    )
}

fn send_reply<T: Serialize>(
    conn: &mut dyn Conn,
    id: Value,
    outcome: rpc::ResponseOutcome<T>,
) -> Result<()> {
    let response = rpc::Response {
        jsonrpc: rpc::JSONRPC.into(),
        id,
        outcome,
    };
    let payload = serde_json::to_vec(&response).context("encode daemon response")?;
    if payload.len() <= CONTROL_MAX {
        return send(conn, payload);
    }
    let response: rpc::Response<()> = rpc::Response {
        jsonrpc: response.jsonrpc,
        id: response.id,
        outcome: rpc::ResponseOutcome::Error {
            error: rpc::Error {
                code: rpc::ErrorCode::TooLarge,
                message: "control_frame_limit".into(),
                data: None,
            },
        },
    };
    send(
        conn,
        serde_json::to_vec(&response).context("encode frame limit error")?,
    )
}

pub(crate) fn notify(conn: &mut dyn Conn, method: &str, params: impl Serialize) -> Result<()> {
    let payload = serde_json::to_vec(&rpc::Notification {
        jsonrpc: rpc::JSONRPC.into(),
        method: method.into(),
        params,
    })
    .context("encode daemon notification")?;
    send(conn, payload)
}

pub(crate) struct WorkerPermit(pub(crate) Arc<Shared>);

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.0.lock().workers -= 1;
        self.0.changed.notify_all();
    }
}
