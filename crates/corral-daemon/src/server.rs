//! Bounded, per-connection RPC workers; the loop never writes to a client socket.
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind, read_frame_until, write_frame_until},
    read::{ReadKind, ReadReply, ReadRequest},
    rpc,
};
use serde::Serialize;
use serde_json::Value;
use std::os::unix::net::{UnixListener, UnixStream};

use crate::state::Shared;

const IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONNECTIONS: usize = 64;
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

#[path = "topics.rs"]
mod topics;

struct WorkerPermit(Arc<Shared>);

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.workers -= 1;
        self.0.changed.notify_all();
    }
}

pub(crate) fn start(listener: UnixListener, shared: Arc<Shared>, store: &Path) -> Result<()> {
    listener
        .set_nonblocking(true)
        .context("setting daemon listener nonblocking")?;
    let store = store.display().to_string();
    std::thread::Builder::new()
        .name("corrald-server".into())
        .spawn(move || {
            while !shared.lock().stop {
                match rustix::event::poll(
                    &mut [rustix::event::PollFd::new(
                        &listener,
                        rustix::event::PollFlags::IN,
                    )],
                    Some(&rustix::event::Timespec {
                        tv_sec: 0,
                        tv_nsec: 500_000_000,
                    }),
                ) {
                    Ok(0) => continue,
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(error) => {
                        shared.fail_server(format!("polling daemon listener: {error}"));
                        break;
                    }
                }
                if shared.lock().stop {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        {
                            let mut state = shared.lock();
                            if state.stop || state.workers >= MAX_CONNECTIONS {
                                continue;
                            }
                            state.workers += 1;
                        }
                        let permit = WorkerPermit(Arc::clone(&shared));
                        let shared = Arc::clone(&shared);
                        let store = store.clone();
                        if let Err(error) = std::thread::Builder::new()
                            .name("corrald-client".into())
                            .spawn(move || {
                                let _permit = permit;
                                if let Err(error) = serve(stream, shared, &store) {
                                    eprintln!("corrald client: {error:#}");
                                }
                            })
                        {
                            eprintln!("corrald accepting client: {error:#}");
                        }
                    }
                    Err(error) => accept_error(&shared, error),
                }
            }
        })
        .context("starting daemon socket server")?;
    Ok(())
}

pub(super) fn accept_error(shared: &Shared, error: std::io::Error) {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::Interrupted | ErrorKind::ConnectionAborted => {}
        _ if matches!(error.raw_os_error(), Some(code)
            if code == rustix::io::Errno::MFILE.raw_os_error()
                || code == rustix::io::Errno::NFILE.raw_os_error()) =>
        {
            eprintln!("corrald accepting client: {error}; retrying after descriptor exhaustion");
            shared.wait(Duration::from_millis(500), false);
        }
        _ => shared.fail_server(format!("accepting daemon connection: {error}")),
    }
}

fn serve(mut stream: UnixStream, shared: Arc<Shared>, store: &str) -> Result<()> {
    check_peer(&stream)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .context("setting client read timeout")?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .context("setting client write timeout")?;
    let mut welcomed = false;
    let hello_deadline = Instant::now() + HELLO_TIMEOUT;
    let mut topics = topics::Topics::new(&shared);
    loop {
        if shared.lock().closed {
            return bye(&mut stream, &shared);
        }
        if !welcomed && Instant::now() >= hello_deadline {
            return Ok(());
        }
        if welcomed {
            topics.flush(&mut stream)?;
        }
        // Poll before reading so idle clients survive between requests, while a partial
        // frame still has a bounded read deadline. Shutdown never waits for the peer.
        let ready = rustix::event::poll(
            &mut [rustix::event::PollFd::new(
                &stream,
                rustix::event::PollFlags::IN,
            )],
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 500_000_000,
            }),
        )
        .context("waiting for client frame")?;
        if ready == 0 {
            continue;
        }
        let deadline = if welcomed {
            Instant::now() + IO_TIMEOUT
        } else {
            hello_deadline.min(Instant::now() + IO_TIMEOUT)
        };
        let frame = match read_frame_until(&mut stream, deadline) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(()),
            Err(error) => {
                if shared.lock().closed {
                    return bye(&mut stream, &shared);
                }
                return Err(error).context("reading client frame");
            }
        };
        ensure!(
            frame.kind == FrameKind::Control && frame.stream == 0,
            "expected control RPC frame"
        );
        let request: rpc::Request =
            serde_json::from_slice(&frame.payload).context("decoding client RPC")?;
        if request.jsonrpc != rpc::JSONRPC {
            error(
                &mut stream,
                request.id,
                rpc::ErrorCode::InvalidParams,
                "expected JSON-RPC 2.0",
            )?;
            continue;
        }
        if !welcomed && request.method != wire::HELLO {
            error(
                &mut stream,
                request.id,
                rpc::ErrorCode::Unauthenticated,
                "hello required",
            )?;
            continue;
        }
        let id = request.id;
        let params = request.params;
        match request.method.as_str() {
            wire::HELLO => match serde_json::from_value::<wire::Hello>(params) {
                Ok(hello) if hello.proto.major == wire::PROTO_MAJOR => {
                    reply(
                        &mut stream,
                        id,
                        wire::Welcome {
                            proto: wire::Proto {
                                major: wire::PROTO_MAJOR,
                                minor: wire::PROTO_MINOR,
                            },
                            caps: hello
                                .caps
                                .into_iter()
                                .filter(|cap| cap == wire::BOARD_V1 || cap == wire::READ_V1)
                                .collect(),
                            daemon: wire::DaemonInfo {
                                version: env!("CARGO_PKG_VERSION").into(),
                                store: store.into(),
                                epoch: shared.epoch(),
                            },
                            health: shared.health(),
                        },
                    )?;
                    welcomed = true;
                }
                Ok(_) => {
                    error(
                        &mut stream,
                        id,
                        rpc::ErrorCode::UnsupportedVersion,
                        "unsupported_version",
                    )?;
                    return Ok(());
                }
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid hello",
                )?,
            },
            wire::PING => reply(&mut stream, id, wire::Pong {})?,
            wire::BYE => {
                reply(
                    &mut stream,
                    id,
                    wire::Bye {
                        reason: "client disconnected".into(),
                    },
                )?;
                return Ok(());
            }
            wire::NUDGE => match serde_json::from_value::<wire::Nudge>(params) {
                Ok(nudge) => {
                    let wait = nudge.wait_ms.map(|ms| Duration::from_millis(u64::from(ms)));
                    reply(&mut stream, id, shared.nudge(wait))?;
                }
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid nudge",
                )?,
            },
            wire::HEALTH_GET => reply(&mut stream, id, shared.health())?,
            wire::READ => match serde_json::from_value::<ReadRequest>(params) {
                Ok(request) => serve_read(&mut stream, &shared, id, request)?,
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid read",
                )?,
            },
            wire::SUB_OPEN => match serde_json::from_value::<wire::subscription::SubOpen>(params) {
                Ok(open) => match topics.open(open) {
                    Ok(()) => reply(&mut stream, id, ())?,
                    Err(problem) => error(
                        &mut stream,
                        id,
                        rpc::ErrorCode::InvalidParams,
                        &problem.to_string(),
                    )?,
                },
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid sub.open",
                )?,
            },
            wire::SUB_ACK => match serde_json::from_value::<wire::subscription::SubAck>(params) {
                Ok(ack) => match topics.ack(ack) {
                    Ok(()) => reply(&mut stream, id, ())?,
                    Err(problem) => error(
                        &mut stream,
                        id,
                        rpc::ErrorCode::InvalidParams,
                        &problem.to_string(),
                    )?,
                },
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid sub.ack",
                )?,
            },
            wire::SUB_CLOSE => match serde_json::from_value::<wire::subscription::SubClose>(params)
            {
                Ok(close) => {
                    topics.close(close);
                    reply(&mut stream, id, ())?;
                }
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid sub.close",
                )?,
            },
            wire::DAEMON_STOP => match serde_json::from_value::<wire::Stop>(params) {
                Ok(stop) => {
                    // Both stop modes finish any already-running pass, never interrupt effects.
                    shared.stop(false);
                    if stop.drain {
                        ensure!(
                            shared.wait_closed(Duration::from_secs(600)),
                            "daemon drain timed out"
                        );
                    }
                    reply(&mut stream, id, wire::Stopped {})?;
                }
                Err(_) => error(
                    &mut stream,
                    id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid stop",
                )?,
            },
            _ => error(
                &mut stream,
                id,
                rpc::ErrorCode::MethodNotFound,
                "method_not_found",
            )?,
        }
    }
}

fn serve_read(
    stream: &mut UnixStream,
    shared: &Shared,
    id: Value,
    request: ReadRequest,
) -> Result<()> {
    if request.kind == ReadKind::Other {
        return error(
            stream,
            id,
            rpc::ErrorCode::InvalidParams,
            "unknown read kind",
        );
    }
    let identity = corral_ops::pm_read::client_identity();
    match identity {
        Ok((exe, build)) if request.exe == exe && request.build == build => {}
        Ok(_) => return error(stream, id, rpc::ErrorCode::Refused, "client_mismatch"),
        Err(problem) => {
            return error(
                stream,
                id,
                rpc::ErrorCode::Internal,
                &format!("{problem:#}").replace(['\r', '\n'], " "),
            );
        }
    }
    let Some(receiver) = crate::reader::request(shared, request.kind) else {
        return error(
            stream,
            id,
            rpc::ErrorCode::Unavailable,
            "reader unavailable",
        );
    };
    let served = match receiver.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(served)) => served,
        Ok(Err(problem)) => {
            return error(
                stream,
                id,
                rpc::ErrorCode::Internal,
                &format!("{problem:#}").replace(['\r', '\n'], " "),
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            return error(stream, id, rpc::ErrorCode::Busy, "reader timed out");
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            return error(
                stream,
                id,
                rpc::ErrorCode::Unavailable,
                "reader unavailable",
            );
        }
    };
    let document = &served.document;
    let frames = u32::try_from(document.bytes.len().div_ceil(wire::frame::DATA_MAX).max(1))
        .context("counting read data frames")?;
    let stream_id = 1;
    reply(
        stream,
        id,
        ReadReply {
            as_of: document.as_of.clone(),
            seq: document.seq,
            cached: served.cached,
            stream: stream_id,
            bytes: document.bytes.len() as u64,
            frames,
        },
    )?;
    // This entire transfer completes before returning to the topic flush / RPC
    // loop. One deadline bounds all DATA frames, not a fresh timeout per frame.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut frame = Frame {
        kind: FrameKind::Data,
        stream: stream_id,
        payload: Vec::with_capacity(document.bytes.len().min(wire::frame::DATA_MAX)),
    };
    if document.bytes.is_empty() {
        write_frame_until(stream, &frame, deadline).context("writing empty read document")?;
    } else {
        for chunk in document.bytes.chunks(wire::frame::DATA_MAX) {
            frame.payload.clear();
            frame.payload.extend_from_slice(chunk);
            write_frame_until(stream, &frame, deadline).context("writing read document")?;
        }
    }
    Ok(())
}

fn send(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let payload = serde_json::to_vec(value).context("encoding daemon response")?;
    write_frame_until(
        stream,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload,
        },
        Instant::now() + IO_TIMEOUT,
    )
    .context("writing daemon response")
}

fn reply(stream: &mut UnixStream, id: Value, result: impl Serialize) -> Result<()> {
    send(
        stream,
        &rpc::Response {
            jsonrpc: rpc::JSONRPC.into(),
            id,
            outcome: rpc::ResponseOutcome::Result { result },
        },
    )
}

fn error(stream: &mut UnixStream, id: Value, code: rpc::ErrorCode, message: &str) -> Result<()> {
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
    send(stream, &response)
}

fn bye(stream: &mut UnixStream, shared: &Shared) -> Result<()> {
    send(
        stream,
        &rpc::Notification {
            jsonrpc: rpc::JSONRPC.into(),
            method: wire::BYE.into(),
            params: wire::Bye {
                reason: shared.bye_reason().into(),
            },
        },
    )
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn check_peer(stream: &UnixStream) -> Result<()> {
    let peer =
        rustix::net::sockopt::socket_peercred(stream).context("reading client peer credentials")?;
    ensure!(
        peer.uid == rustix::process::getuid(),
        "client peer uid does not match this user"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_peer(stream: &UnixStream) -> Result<()> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn getpeereid(fd: i32, uid: *mut u32, gid: *mut u32) -> i32;
    }
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: the borrowed descriptor stays open, and both out-pointers are writable u32s.
    if unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(std::io::Error::last_os_error()).context("reading client peer credentials");
    }
    ensure!(
        uid == rustix::process::getuid().as_raw(),
        "client peer uid does not match this user"
    );
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn check_peer(_stream: &UnixStream) -> Result<()> {
    anyhow::bail!("client peer credentials are unavailable on this platform")
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
