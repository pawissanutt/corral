//! Bounded, per-connection RPC workers; the loop never writes to a client socket.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind, read_frame, write_frame},
    rpc,
};
use serde::Serialize;
use serde_json::Value;
use std::os::unix::net::{UnixListener, UnixStream};

use crate::state::Shared;

const IO_TIMEOUT: Duration = Duration::from_secs(2);

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
                        eprintln!("corrald polling listener: {error:#}");
                        shared.stop(false);
                        break;
                    }
                }
                if shared.lock().stop {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let shared = Arc::clone(&shared);
                        let store = store.clone();
                        if let Err(error) = std::thread::Builder::new()
                            .name("corrald-client".into())
                            .spawn(move || {
                                if let Err(error) = serve(stream, shared, &store) {
                                    eprintln!("corrald client: {error:#}");
                                }
                            })
                        {
                            eprintln!("corrald accepting client: {error:#}");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        eprintln!("corrald listener: {error:#}");
                        shared.stop(false);
                    }
                }
            }
        })
        .context("starting daemon socket server")?;
    Ok(())
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
    loop {
        if shared.lock().closed {
            return bye(&mut stream, &shared);
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
        let frame = match read_frame(&mut stream) {
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
                            caps: Vec::new(),
                            daemon: wire::DaemonInfo {
                                version: env!("CARGO_PKG_VERSION").into(),
                                store: store.into(),
                                epoch: 0,
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

fn send(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let payload = serde_json::to_vec(value).context("encoding daemon response")?;
    write_frame(
        stream,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload,
        },
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
