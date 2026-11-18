//! Authenticated browser sessions share the bounded enrolled-device app gateway.
use std::{
    net::TcpStream,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use corral_protocol::{
    self as wire,
    device::{self, PairState},
    frame::FrameKind,
    rpc::{self, ErrorCode},
};
use serde_json::Value;
use tungstenite::protocol::frame::coding::CloseCode;

use super::{
    dispatch,
    ws::{self, Mode, WsConn},
};
use crate::{
    conn::{error, notify, reply},
    devices::{self, DeviceError, Devices},
    state::Shared,
    topics::Subscriptions,
};

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);
const MESSAGE_LIMIT: u64 = 1 << 20;

// This guard covers every return path, including failed replies and malformed RPCs.
// Pairings::gone leaves Confirming/Paired intact, so a committed device cannot be undone.
struct PairGuard {
    devices: Arc<Devices>,
    pair_id: String,
}

impl Drop for PairGuard {
    fn drop(&mut self) {
        self.devices.pairings.gone(&self.pair_id);
    }
}

pub(crate) fn serve(stream: TcpStream, shared: Arc<Shared>, origins: &[String]) -> Result<()> {
    let devices = shared.devices();
    let (socket, mode) = ws::upgrade(stream, origins)?;
    let established = ws::handshake(socket, mode, devices)?;
    let mut conn = established.conn;
    let session = established.session;
    if session.is_some() {
        shared.publish();
    }
    let pair = match established.mode {
        Mode::Session => {
            if session.is_none() {
                conn.close(CloseCode::Policy, device::CLOSE_DEVICE_UNKNOWN);
                anyhow::bail!("app session has no authenticated device");
            }
            None
        }
        Mode::Pair(pair_id) => Some(PairGuard {
            devices: Arc::clone(devices),
            pair_id,
        }),
    };
    let mut welcomed = false;
    let mut claimed = false;
    let hello_deadline = Instant::now() + HELLO_TIMEOUT;
    let mut topics = Subscriptions::new(&shared);
    loop {
        if shared.lock().closed {
            conn.close(CloseCode::Away, device::CLOSE_DAEMON_STOPPING);
            return Ok(());
        }
        if session
            .as_ref()
            .is_some_and(|guard| guard.revoked.load(Ordering::Acquire))
        {
            conn.close(CloseCode::Policy, device::CLOSE_REVOKED);
            return Ok(());
        }
        if conn.opened.elapsed() >= SESSION_LIFETIME || conn.messages >= MESSAGE_LIMIT {
            conn.close(CloseCode::Normal, device::CLOSE_SESSION_LIFETIME);
            return Ok(());
        }
        if !welcomed && Instant::now() >= hello_deadline {
            conn.close(CloseCode::Policy, device::CLOSE_PROTOCOL);
            return Ok(());
        }
        if let Some(pair) = &pair {
            if pairing_finished(&mut conn, pair)? {
                return Ok(());
            }
        } else if welcomed {
            if let Some(guard) = &session {
                guard.touch()?;
            }
            topics.flush(&mut conn)?;
        }
        let frame = match conn.recv() {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(problem) if ws::peer_gone(&problem) => return Ok(()),
            Err(problem) => {
                conn.close(CloseCode::Policy, device::CLOSE_PROTOCOL);
                return Err(problem);
            }
        };
        // Receiving another RPC must never renew either the hello or pairing deadline.
        if !welcomed && Instant::now() >= hello_deadline {
            conn.close(CloseCode::Policy, device::CLOSE_PROTOCOL);
            return Ok(());
        }
        if frame.kind != FrameKind::Control
            || frame.stream != 0
            || frame.payload.len() > wire::app::APP_CONTROL_MAX
        {
            conn.close(CloseCode::Policy, device::CLOSE_PROTOCOL);
            return Ok(());
        }
        let request: rpc::Request = match serde_json::from_slice(&frame.payload) {
            Ok(request) => request,
            Err(_) => {
                conn.close(CloseCode::Policy, device::CLOSE_PROTOCOL);
                return Ok(());
            }
        };
        if serde_json::to_vec(&request.id)?.len() > 256 {
            error(
                &mut conn,
                Value::Null,
                ErrorCode::InvalidParams,
                "invalid_rpc_id",
            )?;
            continue;
        }
        if request.jsonrpc != rpc::JSONRPC {
            error(
                &mut conn,
                request.id,
                ErrorCode::InvalidParams,
                "expected JSON-RPC 2.0",
            )?;
            continue;
        }
        if !welcomed && request.method != wire::HELLO {
            error(
                &mut conn,
                request.id,
                ErrorCode::Unauthenticated,
                "hello required",
            )?;
            continue;
        }
        if request.method == wire::HELLO && !claimed {
            let id = request.id;
            match serde_json::from_value::<wire::Hello>(request.params) {
                Ok(mut hello) if hello.proto.major == wire::PROTO_MAJOR => {
                    if pair.is_some() {
                        hello.caps.clear();
                    }
                    topics.set_session_fleet(
                        hello
                            .caps
                            .iter()
                            .any(|cap| cap == wire::app::SESSION_FLEET_V1),
                    );
                    reply(&mut conn, id, dispatch::welcome(&shared, hello))?;
                    welcomed = true;
                }
                Ok(_) => {
                    error(
                        &mut conn,
                        id,
                        ErrorCode::UnsupportedVersion,
                        "unsupported_version",
                    )?;
                    conn.close(CloseCode::Policy, device::CLOSE_PROTOCOL);
                    return Ok(());
                }
                Err(_) => error(&mut conn, id, ErrorCode::InvalidParams, "invalid hello")?,
            }
            continue;
        }
        if let Some(pair) = &pair {
            let id = request.id;
            if request.method == device::PAIR_CLAIM && !claimed {
                match serde_json::from_value::<device::PairClaim>(request.params) {
                    Ok(claim) => {
                        let result = devices::validate(&claim.name, &claim.app_version).and_then(
                            |(name, version)| {
                                devices
                                    .pairings
                                    .claim(&pair.pair_id, name, version, Instant::now())
                                    .map_err(DeviceError::from)
                            },
                        );
                        match result {
                            Ok(sas) => {
                                claimed = true;
                                reply(&mut conn, id, device::PairClaimed { sas })?;
                            }
                            Err(problem) => {
                                let (code, message) = problem.wire();
                                error(&mut conn, id, code, &message)?;
                            }
                        }
                    }
                    Err(_) => error(
                        &mut conn,
                        id,
                        ErrorCode::InvalidParams,
                        "invalid pair.claim",
                    )?,
                }
            } else {
                refused(&mut conn, id)?;
            }
            continue;
        }
        if request.method == wire::BYE {
            reply(
                &mut conn,
                request.id,
                wire::Bye {
                    reason: "client disconnected".into(),
                },
            )?;
            conn.close(CloseCode::Normal, "client disconnected");
            return Ok(());
        }
        dispatch::serve(&mut conn, &shared, request, session.as_ref(), &mut topics)?;
    }
}

fn pairing_finished(conn: &mut WsConn, pair: &PairGuard) -> Result<bool> {
    match pair
        .devices
        .pairings
        .status(&pair.pair_id, Instant::now(), Duration::ZERO)
    {
        Ok(PairState::Paired { device: info }) => {
            let result = notify(
                conn,
                device::PAIR_DONE,
                device::PairDone { device: info.id },
            );
            conn.close(CloseCode::Normal, device::CLOSE_PAIRED);
            result?;
            Ok(true)
        }
        Ok(PairState::Ended { reason }) => {
            let result = notify(
                conn,
                device::PAIR_REFUSED,
                device::PairRefused {
                    reason: reason.clone(),
                },
            );
            conn.close(CloseCode::Policy, &reason);
            result?;
            Ok(true)
        }
        Ok(PairState::Offered { .. } | PairState::Claimed { .. }) => Ok(false),
        Ok(PairState::Other) | Err(_) => {
            conn.close(CloseCode::Policy, device::CLOSE_PAIRING_UNAVAILABLE);
            Ok(true)
        }
    }
}

fn refused(conn: &mut WsConn, id: Value) -> Result<()> {
    error(conn, id, ErrorCode::Refused, "app_method_refused")
}
