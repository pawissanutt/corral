//! The local control client used by corrald-next subcommands.
use std::{
    io::ErrorKind,
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind, read_frame_until, write_frame_until},
    rpc,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub struct Client {
    stream: UnixStream,
    id: u64,
}

impl Client {
    pub fn connect(root: &Path) -> Result<Option<Self>> {
        let path = crate::socket_path(root);
        let stream = match UnixStream::connect(&path) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("connect to {}", path.display()));
            }
        };
        let mut client = Self { stream, id: 0 };
        let _: wire::Welcome = client.request(
            wire::HELLO,
            wire::Hello {
                proto: wire::Proto {
                    major: wire::PROTO_MAJOR,
                    minor: wire::PROTO_MINOR,
                },
                caps: Vec::new(),
                client: wire::ClientInfo {
                    kind: "cli".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            },
            Duration::from_secs(2),
        )?;
        Ok(Some(client))
    }

    pub fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: impl Serialize,
        timeout: Duration,
    ) -> Result<T> {
        self.id += 1;
        let id = Value::from(self.id);
        let payload = serde_json::to_vec(&rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            id: id.clone(),
            method: method.into(),
            params,
        })
        .with_context(|| format!("encode {method} request"))?;
        let deadline = Instant::now() + timeout;
        write_frame_until(
            &mut self.stream,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload,
            },
            deadline,
        )
        .with_context(|| format!("send {method} request"))?;
        let frame = read_frame_until(&mut self.stream, deadline)
            .with_context(|| format!("read {method} response"))?
            .context("daemon disconnected")?;
        ensure!(
            frame.kind == FrameKind::Control && frame.stream == 0,
            "expected daemon control response"
        );
        let response: rpc::Response<T> = serde_json::from_slice(&frame.payload)
            .with_context(|| format!("decode {method} response"))?;
        ensure!(
            response.jsonrpc == rpc::JSONRPC && response.id == id,
            "unexpected daemon response to {method}"
        );
        match response.outcome {
            rpc::ResponseOutcome::Result { result } => Ok(result),
            rpc::ResponseOutcome::Error { error } => anyhow::bail!("{method}: {}", error.message),
        }
    }
}
