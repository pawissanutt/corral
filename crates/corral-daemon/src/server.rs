//! Bounded, per-connection RPC workers; the loop never writes to a client socket.
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    frame::{FrameKind, read_frame_until},
    rpc,
};
use std::os::unix::net::{UnixListener, UnixStream};

use crate::{
    app::dispatch,
    conn::{IO_TIMEOUT, Peer, WorkerPermit, error, notify, reply},
    state::Shared,
    topics::Subscriptions,
};

pub(crate) const MAX_CONNECTIONS: usize = 64;
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn start(listener: UnixListener, shared: Arc<Shared>) -> Result<()> {
    listener
        .set_nonblocking(true)
        .context("setting daemon listener nonblocking")?;
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
                        if let Err(error) = std::thread::Builder::new()
                            .name("corrald-client".into())
                            .spawn(move || {
                                let _permit = permit;
                                if let Err(error) = serve(stream, shared) {
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
            shared.wait(Duration::from_millis(500));
        }
        _ => shared.fail_server(format!("accepting daemon connection: {error}")),
    }
}

pub(super) fn serve(mut stream: UnixStream, shared: Arc<Shared>) -> Result<()> {
    check_peer(&stream)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .context("setting client read timeout")?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .context("setting client write timeout")?;
    let mut welcomed = false;
    let hello_deadline = Instant::now() + HELLO_TIMEOUT;
    let mut topics = Subscriptions::new(&shared);
    let mut attached: Option<crate::devices::SessionGuard> = None;
    loop {
        if attached
            .as_ref()
            .is_some_and(|guard| guard.revoked.load(std::sync::atomic::Ordering::Acquire))
        {
            return notify(
                &mut stream,
                wire::BYE,
                wire::Bye {
                    reason: wire::device::CLOSE_REVOKED.into(),
                },
            );
        }
        if shared.lock().closed {
            return bye(&mut stream);
        }
        if !welcomed && Instant::now() >= hello_deadline {
            return Ok(());
        }
        if welcomed {
            if let Some(guard) = &attached {
                guard.touch()?;
            }
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
                    return bye(&mut stream);
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
        if serde_json::to_vec(&request.id)
            .context("encode client request ID")?
            .len()
            > 256
        {
            error(
                &mut stream,
                serde_json::Value::Null,
                rpc::ErrorCode::TooLarge,
                "rpc_id_limit",
            )?;
            continue;
        }
        match request.method.as_str() {
            wire::HELLO => match serde_json::from_value::<wire::Hello>(request.params) {
                Ok(hello) if hello.proto.major == wire::PROTO_MAJOR => {
                    topics.set_capabilities(&hello.caps);
                    reply(&mut stream, request.id, dispatch::welcome(&shared, hello))?;
                    welcomed = true;
                }
                Ok(_) => {
                    error(
                        &mut stream,
                        request.id,
                        rpc::ErrorCode::UnsupportedVersion,
                        "unsupported_version",
                    )?;
                    return Ok(());
                }
                Err(_) => error(
                    &mut stream,
                    request.id,
                    rpc::ErrorCode::InvalidParams,
                    "invalid hello",
                )?,
            },
            wire::BYE => {
                reply(
                    &mut stream,
                    request.id,
                    wire::Bye {
                        reason: "client disconnected".into(),
                    },
                )?;
                return Ok(());
            }
            wire::device::DEVICE_ENROL | wire::device::DEVICE_ATTACH => {
                crate::devices::serve_unix(
                    &mut stream,
                    &shared,
                    request.id,
                    &request.method,
                    request.params,
                    &mut attached,
                )?;
            }
            _ => dispatch::serve(
                &mut stream,
                &Peer::Unix,
                &shared,
                request,
                attached.as_ref(),
                &mut topics,
            )?,
        }
    }
}

fn bye(stream: &mut UnixStream) -> Result<()> {
    notify(
        stream,
        wire::BYE,
        wire::Bye {
            reason: "daemon stopped".into(),
        },
    )
}

/// The verified peer uid: always this daemon's own uid.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn check_peer(stream: &UnixStream) -> Result<u32> {
    let peer =
        rustix::net::sockopt::socket_peercred(stream).context("reading client peer credentials")?;
    ensure!(
        peer.uid == rustix::process::getuid(),
        "client peer uid does not match this user"
    );
    Ok(peer.uid.as_raw())
}

#[cfg(target_os = "macos")]
fn check_peer(stream: &UnixStream) -> Result<u32> {
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
    Ok(uid)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn check_peer(_stream: &UnixStream) -> Result<u32> {
    anyhow::bail!("client peer credentials are unavailable on this platform")
}
