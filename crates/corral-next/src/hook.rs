//! A herdr hook only asks the daemon to sync; CLI operations otherwise go to the engine.
use std::path::Path;

use anyhow::Result;

#[cfg(unix)]
pub(crate) fn nudge(root: &Path) -> Result<bool> {
    use anyhow::{Context, ensure};
    use corral_protocol::{
        self as wire,
        frame::{Frame, FrameKind, read_frame_until, write_frame_until},
        rpc,
    };
    use serde_json::json;
    use std::{
        io::ErrorKind,
        os::unix::net::UnixStream,
        time::{Duration, Instant},
    };

    let path = root.join(".corral-next/daemon.sock");
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error).with_context(|| format!("connect to {}", path.display())),
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    for (id, method, params) in [
        (
            1,
            wire::HELLO,
            json!(wire::Hello {
                proto: wire::Proto {
                    major: wire::PROTO_MAJOR,
                    minor: wire::PROTO_MINOR
                },
                caps: Vec::new(),
                client: wire::ClientInfo {
                    kind: "cli".into(),
                    version: env!("CARGO_PKG_VERSION").into()
                },
            }),
        ),
        (
            2,
            wire::NUDGE,
            json!(wire::Nudge {
                source: "hook".into(),
                wait_ms: None
            }),
        ),
    ] {
        let id = json!(id);
        let payload = serde_json::to_vec(&rpc::Request {
            jsonrpc: rpc::JSONRPC.into(),
            id: id.clone(),
            method: method.into(),
            params,
        })
        .with_context(|| format!("encode hook {method}"))?;
        write_frame_until(
            &mut stream,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload,
            },
            deadline,
        )
        .with_context(|| format!("send hook {method}"))?;
        let frame = read_frame_until(&mut stream, deadline)
            .with_context(|| format!("read hook {method}"))?
            .context("daemon disconnected")?;
        ensure!(
            frame.kind == FrameKind::Control && frame.stream == 0,
            "invalid hook response frame"
        );
        let reply: rpc::Response = serde_json::from_slice(&frame.payload)
            .with_context(|| format!("decode hook {method} response"))?;
        ensure!(
            reply.jsonrpc == rpc::JSONRPC && reply.id == id,
            "unexpected hook {method} response"
        );
        if let rpc::ResponseOutcome::Error { error } = reply.outcome {
            anyhow::bail!("hook {method}: {}", error.message);
        }
    }
    Ok(true)
}

#[cfg(not(unix))]
pub(crate) fn nudge(_root: &Path) -> Result<bool> {
    Ok(false)
}
