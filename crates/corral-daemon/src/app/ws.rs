//! Bounded browser upgrade, authenticated Noise handshake, and encrypted frames.
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use corral_protocol::{
    device,
    frame::{Frame, MESSAGE_MAX, decode_message, encode_message},
};
use snow::{Builder, TransportState};
use tungstenite::{
    Error as WsError, Message, WebSocket, accept_hdr_with_config,
    error::ProtocolError,
    handshake::server::{ErrorResponse, Request, Response},
    http::StatusCode,
    protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
};

use crate::{
    conn::{Conn, IO_TIMEOUT},
    devices::{Devices, SessionGuard},
};

const UPGRADE_MAX: usize = 8_192;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const READ_SLICE: Duration = Duration::from_millis(250);
const CIPHER_MAX: usize = MESSAGE_MAX + device::NOISE_TAG;

pub(super) enum Mode {
    Session,
    Pair(String),
}

/// Deadlines apply to the entire operation, including fragmented TCP reads.
pub(super) struct Bounded {
    stream: TcpStream,
    remaining: Option<usize>,
    read_deadline: Instant,
    write_deadline: Instant,
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "app I/O deadline elapsed"))
}

impl Bounded {
    fn budget(&mut self, read: Duration, write: Duration) {
        let now = Instant::now();
        self.read_deadline = now + read;
        self.write_deadline = now + write;
    }
}

impl Read for Bounded {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.stream
            .set_read_timeout(Some(remaining(self.read_deadline)?))?;
        let len = match self.remaining {
            Some(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "app HTTP upgrade exceeds 8192 bytes",
                ));
            }
            Some(limit) => bytes.len().min(limit),
            None => bytes.len(),
        };
        let read = self.stream.read(&mut bytes[..len])?;
        if let Some(limit) = &mut self.remaining {
            *limit -= read;
        }
        Ok(read)
    }
}

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream
            .set_write_timeout(Some(remaining(self.write_deadline)?))?;
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        remaining(self.write_deadline)?;
        self.stream.flush()
    }
}

fn reject(status: StatusCode) -> ErrorResponse {
    let mut response = ErrorResponse::new(None);
    *response.status_mut() = status;
    response
}

fn origin_allowed(headers: &tungstenite::http::HeaderMap, origins: &[String]) -> bool {
    let mut values = headers.get_all("Origin").iter();
    let allowed = values.next().is_some_and(|value| {
        origins
            .iter()
            .any(|origin| value.as_bytes() == origin.as_bytes())
    });
    allowed && values.next().is_none()
}

pub(super) fn upgrade(stream: TcpStream, origins: &[String]) -> Result<(WebSocket<Bounded>, Mode)> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let stream = Bounded {
        stream,
        remaining: Some(UPGRADE_MAX),
        read_deadline: deadline,
        write_deadline: deadline,
    };
    let mut mode = None;
    #[expect(
        clippy::result_large_err,
        reason = "tungstenite fixes the callback's error type to an HTTP response"
    )]
    let callback = |request: &Request, response: Response| {
        if !origin_allowed(request.headers(), origins) {
            return Err(reject(StatusCode::FORBIDDEN));
        }
        // Comparing the complete path-and-query also refuses query suffixes.
        let path = request
            .uri()
            .path_and_query()
            .map_or("", |path| path.as_str());
        mode = if path == device::SESSION_PATH {
            Some(Mode::Session)
        } else if let Some(pair) = path.strip_prefix(device::PAIR_PATH_PREFIX) {
            if pair.len() != 16
                || !pair
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(reject(StatusCode::NOT_FOUND));
            }
            Some(Mode::Pair(pair.to_owned()))
        } else {
            return Err(reject(StatusCode::NOT_FOUND));
        };
        Ok(response)
    };
    let config = WebSocketConfig::default()
        .read_buffer_size(UPGRADE_MAX)
        .write_buffer_size(0)
        .max_write_buffer_size(CIPHER_MAX + 512)
        .max_message_size(Some(128))
        .max_frame_size(Some(128));
    let mut ws = accept_hdr_with_config(stream, callback, Some(config))
        .map_err(|error| anyhow!("app WebSocket upgrade: {error}"))?;
    remaining(deadline)?;
    ws.get_mut().remaining = None;
    Ok((ws, mode.context("app WebSocket path was not selected")?))
}

pub(super) struct Established {
    pub(super) conn: WsConn,
    pub(super) mode: Mode,
    pub(super) session: Option<SessionGuard>,
}

/// Only the successful claimant owns cleanup; a second handshake must not end it.
struct PairBurn<'a> {
    devices: &'a Devices,
    pair: Option<&'a str>,
}

impl Drop for PairBurn<'_> {
    fn drop(&mut self) {
        if let Some(pair) = self.pair {
            self.devices.pairings.gone(pair);
        }
    }
}

pub(super) fn handshake(
    mut ws: WebSocket<Bounded>,
    mode: Mode,
    devices: &Arc<Devices>,
) -> Result<Established> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let reach = crate::devices::reach::classify(ws.get_ref().stream.local_addr()?.ip());
    ws.get_mut().budget(HANDSHAKE_TIMEOUT, HANDSHAKE_TIMEOUT);
    let mut reason = device::CLOSE_HANDSHAKE_FAILED;
    let mut burn = PairBurn {
        devices,
        pair: None,
    };
    let result = (|| -> Result<(TransportState, Option<SessionGuard>)> {
        let identity = devices
            .identity()
            .map_err(|error| anyhow!(error.wire().1))?;
        // Check the single-use offer before reading attacker-controlled Noise bytes.
        let token = match &mode {
            Mode::Session => None,
            Mode::Pair(pair) => Some(devices.pairings.token(pair, Instant::now()).map_err(
                |error| {
                    reason = device::CLOSE_PAIRING_UNAVAILABLE;
                    anyhow!("pairing unavailable: {error:?}")
                },
            )?),
        };
        let pattern = if token.is_some() {
            device::NOISE_PAIR
        } else {
            device::NOISE_SESSION
        };
        let builder = Builder::new(pattern.parse()?).local_private_key(&identity.private)?;
        let builder = if let Some(token) = &token {
            builder.psk(2, token)?
        } else {
            builder
        };
        let mut noise = builder.build_responder()?;
        let first = match ws.read()? {
            Message::Binary(bytes) if bytes.len() == device::HANDSHAKE_FIRST_LEN => bytes,
            _ => bail!("expected the empty-payload Noise first message"),
        };
        let mut payload = [0; 128];
        ensure!(
            noise.read_message(&first, &mut payload)? == 0,
            "Noise handshake payload refused"
        );
        let public: [u8; 32] = noise
            .get_remote_static()
            .context("Noise remote static missing")?
            .try_into()
            .context("Noise remote static length")?;
        remaining(deadline)?;
        let session = match &mode {
            Mode::Session => Some(
                devices
                    .attach_key(&public, reach)
                    .map_err(|error| {
                        reason = device::CLOSE_DEVICE_UNKNOWN;
                        anyhow!(error.wire().1)
                    })?
                    .1,
            ),
            Mode::Pair(_) => None,
        };
        let mut response = vec![0; 128];
        let len = noise.write_message(&[], &mut response)?;
        response.truncate(len);
        // IK's final hash includes the responder message, even with an empty payload.
        if let Mode::Pair(pair) = &mode {
            devices
                .pairings
                .handshaken(
                    pair,
                    public,
                    device::sas(noise.get_handshake_hash()),
                    Instant::now(),
                )
                .map_err(|error| {
                    reason = device::CLOSE_PAIRING_UNAVAILABLE;
                    anyhow!("pairing unavailable: {error:?}")
                })?;
            burn.pair = Some(pair);
        }
        let transport = noise.into_transport_mode()?;
        remaining(deadline)?;
        ws.send(Message::Binary(response.into()))?;
        remaining(deadline)?;
        Ok((transport, session))
    })();
    let (noise, session) = match result {
        Ok(established) => established,
        Err(error) => {
            // Retain the original elapsed budget, including this best-effort close.
            let _ = ws.close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: reason.into(),
            }));
            let _ = ws.flush();
            return Err(error);
        }
    };
    ws.set_config(|config| {
        config.max_message_size = Some(CIPHER_MAX);
        config.max_frame_size = Some(CIPHER_MAX);
    });
    ws.get_mut().budget(READ_SLICE, IO_TIMEOUT);
    ws.get_mut().stream.set_read_timeout(Some(READ_SLICE))?;
    ws.get_mut().stream.set_write_timeout(Some(IO_TIMEOUT))?;
    burn.pair = None;
    drop(burn);
    Ok(Established {
        conn: WsConn {
            ws,
            noise,
            plain: vec![0; MESSAGE_MAX],
            cipher: Vec::new(),
            messages: 0,
            opened: Instant::now(),
        },
        mode,
        session,
    })
}

pub(super) struct WsConn {
    ws: WebSocket<Bounded>,
    noise: TransportState,
    plain: Vec<u8>,
    cipher: Vec<u8>,
    pub(super) messages: u64,
    pub(super) opened: Instant,
}

impl Conn for WsConn {
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let plain = encode_message(frame)?;
        self.cipher.resize(plain.len() + device::NOISE_TAG, 0);
        let len = self.noise.write_message(&plain, &mut self.cipher)?;
        self.cipher.truncate(len);
        self.ws.get_mut().write_deadline = Instant::now() + IO_TIMEOUT;
        // Transfer the ciphertext allocation to tungstenite rather than copying it.
        self.ws
            .send(Message::Binary(std::mem::take(&mut self.cipher).into()))?;
        self.messages += 1;
        Ok(())
    }
}

impl WsConn {
    pub(super) fn recv(&mut self) -> Result<Option<Frame>> {
        self.ws.get_mut().budget(READ_SLICE, IO_TIMEOUT);
        let deadline = self.ws.get_mut().read_deadline;
        loop {
            if Instant::now() >= deadline {
                return Ok(None);
            }
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    self.messages += 1;
                    ensure!(
                        (device::NOISE_TAG + 5..=CIPHER_MAX).contains(&bytes.len()),
                        "app ciphertext length refused"
                    );
                    let len = self.noise.read_message(&bytes, &mut self.plain)?;
                    return Ok(Some(decode_message(&self.plain[..len])?));
                }
                Ok(Message::Ping(_) | Message::Pong(_)) => {
                    // Tungstenite queues pong replies; flush them within this slice's budget.
                    self.ws.flush()?;
                }
                Ok(Message::Close(_)) => {
                    let _ = self.ws.flush();
                    return Err(WsError::ConnectionClosed.into());
                }
                Ok(_) => bail!("app accepts only binary encrypted messages"),
                Err(WsError::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub(super) fn close(&mut self, code: CloseCode, reason: &str) {
        self.ws.get_mut().write_deadline = Instant::now() + IO_TIMEOUT;
        let _ = self.ws.close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }));
        let _ = self.ws.flush();
    }
}

pub(super) fn peer_gone(error: &anyhow::Error) -> bool {
    match error.downcast_ref::<WsError>() {
        Some(WsError::ConnectionClosed | WsError::AlreadyClosed) => true,
        Some(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => true,
        Some(WsError::Io(error)) => matches!(
            error.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::NotConnected
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_origins_need_one_exact_allowlisted_header_before_upgrade() {
        let origins = vec!["https://workstation.tailnet.ts.net".to_owned()];
        let mut headers = tungstenite::http::HeaderMap::new();
        assert!(!origin_allowed(&headers, &origins));
        for origin in [
            "null",
            "http://workstation.tailnet.ts.net",
            "https://evil.example",
            "https://workstation.tailnet.ts.net.evil",
        ] {
            headers.insert("Origin", origin.parse().unwrap());
            assert!(!origin_allowed(&headers, &origins));
        }
        headers.insert("Origin", origins[0].parse().unwrap());
        assert!(origin_allowed(&headers, &origins));
        headers.append("Origin", origins[0].parse().unwrap());
        assert!(!origin_allowed(&headers, &origins));
    }
}
