//! Browser-owned Noise sessions. Only the non-secret paired marker uses NSH Storage.
mod keystore;
mod noise;
mod socket;

use super::{
    PAIRED_KEY,
    core::{Core, Event},
    map,
    requests::ClientRequest,
};
use crate::model::{
    daemon::Feed,
    devices::{BrowserPairing, BrowserSessionClose, DAEMON_IDENTITY_FAILED, DeviceLink},
};
use corral_protocol::{
    self as wire,
    device::*,
    frame::{MESSAGE_MAX, decode_message, encode_message},
    subscription::Topic,
};
use futures_channel::mpsc::{self, UnboundedSender};
use futures_core::Stream;
use keystore::WebCredential;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;
use serde_json::json;
use snow::{HandshakeState, TransportState};
use socket::Socket;
use std::{cell::RefCell, pin::Pin, time::Duration};

pub(super) enum Input {
    Opened {
        generation: u64,
    },
    Message {
        generation: u64,
        bytes: Vec<u8>,
    },
    Closed {
        generation: u64,
        code: u16,
        reason: String,
    },
    Request(ClientRequest),
    Tick,
}
thread_local! { static SENDER: RefCell<Option<UnboundedSender<Input>>> = const { RefCell::new(None) }; }

pub(super) fn send(request: ClientRequest) -> bool {
    SENDER.with(|sender| {
        sender
            .borrow()
            .as_ref()
            .is_some_and(|sender| sender.unbounded_send(Input::Request(request)).is_ok())
    })
}

pub(super) fn start(feed: Signal<Feed>) {
    let (sender, mut receiver) = mpsc::unbounded();
    SENDER.with(|slot| *slot.borrow_mut() = Some(sender.clone()));
    let ticks = sender.clone();
    spawn(async move {
        loop {
            timer(Duration::from_secs(1)).await;
            if ticks.unbounded_send(Input::Tick).is_err() {
                break;
            }
        }
    });
    spawn(async move {
        let Some(clock) = web_sys::window().and_then(|window| window.performance()) else {
            feed.update(|feed| {
                feed.device = DeviceLink::Failed(
                    "Browser timing is unavailable; use a supported secure browser.".into(),
                )
            });
            return;
        };
        let mut driver = Driver {
            feed,
            sender,
            clock,
            credential: None,
            connection: None,
            generation: 0,
            failures: 0,
            retry_at: None,
        };
        driver.tick_clock();
        if super::configured() && !feed.with_untracked(Feed::browser_pairing_pending) {
            match keystore::load().await {
                Ok(Some(credential)) => {
                    driver.credential = Some(credential);
                    driver.connect_session();
                }
                Ok(None) => {
                    Storage::remove_item(PAIRED_KEY);
                    feed.update(|feed| {
                        feed.device = DeviceLink::Failed(
                            "This browser has no stored pairing; pair it again.".into(),
                        )
                    });
                }
                Err(problem) => feed.update(|feed| feed.device = DeviceLink::Failed(problem)),
            }
        }
        while let Some(input) =
            std::future::poll_fn(|cx| Pin::new(&mut receiver).poll_next(cx)).await
        {
            driver.handle(input).await;
        }
    });
}

enum Role {
    Session,
    Pair {
        credential: WebCredential,
        sas: Option<String>,
    },
}
enum Stage {
    Opening,
    Handshake,
    Transport,
}
struct Connection {
    socket: Socket,
    generation: u64,
    role: Role,
    stage: Stage,
    handshake: Option<HandshakeState>,
    noise: Option<TransportState>,
    core: Core,
    started: u64,
    received: u64,
    pinged: u64,
    plain: Vec<u8>,
    cipher: Vec<u8>,
}
impl Connection {
    fn flush(&mut self) -> Result<(), String> {
        let noise = self
            .noise
            .as_mut()
            .ok_or("Noise transport is not established")?;
        while let Some(frame) = self.core.take_outbox() {
            let plain = encode_message(&frame).map_err(|error| error.to_string())?;
            self.cipher.resize(plain.len() + NOISE_TAG, 0);
            let len = noise
                .write_message(&plain, &mut self.cipher)
                .map_err(|error| format!("cannot encrypt daemon frame: {error}"))?;
            self.socket.send(&self.cipher[..len])?;
        }
        Ok(())
    }
}

struct Driver {
    feed: Signal<Feed>,
    sender: UnboundedSender<Input>,
    clock: web_sys::Performance,
    credential: Option<WebCredential>,
    connection: Option<Connection>,
    generation: u64,
    failures: usize,
    retry_at: Option<u64>,
}
enum Action {
    Keep,
    Paired(WebCredential),
    Stop,
}
enum Failure {
    Retry(String),
    Fatal(String),
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Retry(message)
    }
}

impl Driver {
    fn now(&self) -> u64 {
        self.clock.now() as u64
    }

    fn tick_clock(&self) {
        let local = jiff::Zoned::now();
        let seconds = u64::try_from(local.timestamp().as_second()).unwrap_or(0);
        if self.feed.with_untracked(|feed| feed.now != seconds) {
            self.feed.update(|feed| {
                feed.tick(
                    seconds,
                    format!("{:02}:{:02}", local.hour(), local.minute()),
                )
            });
        }
    }

    fn open(
        &mut self,
        port: u16,
        path: &str,
        role: Role,
        handshake: HandshakeState,
    ) -> Result<(), String> {
        self.connection = None;
        self.retry_at = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let socket = Socket::open(port, path, generation, self.sender.clone())?;
        let now = self.now();
        self.connection = Some(Connection {
            socket,
            generation,
            role,
            stage: Stage::Opening,
            handshake: Some(handshake),
            noise: None,
            core: Core::new(),
            started: now,
            received: now,
            pinged: now,
            plain: Vec::new(),
            cipher: Vec::new(),
        });
        Ok(())
    }

    fn connect_session(&mut self) {
        let result = (|| {
            let credential = self
                .credential
                .as_ref()
                .ok_or("This browser is not paired")?;
            let port = credential.port;
            let private = key(&credential.private)?;
            let daemon = key(&credential.daemon)?;
            let handshake = noise::initiator_session(&private, &daemon)?;
            self.feed
                .update(|feed| feed.connecting(format!("ws://127.0.0.1:{port}")));
            self.open(port, SESSION_PATH, Role::Session, handshake)
        })();
        if let Err(problem) = result {
            self.lost(problem);
        }
    }

    fn lost(&mut self, problem: String) {
        self.connection = None;
        self.feed.update(|feed| feed.link_lost(problem));
        let seconds = match self.failures {
            0 => 1,
            1 => 2,
            2 => 5,
            _ => 10,
        };
        self.failures = self.failures.saturating_add(1);
        self.retry_at = self
            .credential
            .as_ref()
            .map(|_| self.now() + seconds * 1_000);
    }

    fn failed(&mut self, pair: bool, failure: Failure) {
        self.connection = None;
        self.retry_at = None;
        match failure {
            Failure::Retry(problem) if !pair => self.lost(problem),
            Failure::Retry(problem) | Failure::Fatal(problem) => self.feed.update(|feed| {
                if pair {
                    feed.browser_pairing = BrowserPairing::Failed(problem);
                } else {
                    feed.device = DeviceLink::Failed(problem.clone());
                    feed.link_lost(problem);
                }
            }),
        }
    }

    async fn handle(&mut self, input: Input) {
        match input {
            Input::Request(ClientRequest::Forget) => {
                self.connection = None;
                self.retry_at = None;
                self.credential = None;
                let blocked = self.feed.with_untracked(|feed| match feed.browser_pairing {
                    BrowserPairing::Blocked(line) => Some(line),
                    _ => None,
                });
                let cleared = keystore::clear().await;
                Storage::remove_item(PAIRED_KEY);
                self.feed.update(|feed| {
                    *feed = Feed::default();
                    if let Some(line) = blocked {
                        feed.browser_pairing = BrowserPairing::Blocked(line);
                    } else if let Err(problem) = cleared {
                        feed.browser_pairing = BrowserPairing::Failed(problem);
                    }
                });
            }
            Input::Request(ClientRequest::Pair { code, name }) => {
                if self.feed.with_untracked(|feed| {
                    matches!(feed.browser_pairing, BrowserPairing::Blocked(_))
                }) {
                    return;
                }
                self.connection = None;
                self.retry_at = None;
                self.feed
                    .update(|feed| feed.browser_pairing = BrowserPairing::Connecting);
                let result = (|| {
                    let code = PairCode::decode(&code)?;
                    let keys = noise::generate_keypair()?;
                    let private: [u8; 32] = keys
                        .private
                        .as_slice()
                        .try_into()
                        .map_err(|_| "invalid browser private key")?;
                    let handshake =
                        noise::initiator_pair(&private, &code.daemon_public, &code.token)?;
                    let credential = WebCredential {
                        v: 1,
                        device: String::new(),
                        name,
                        private: hex(&private),
                        daemon: hex(&code.daemon_public),
                        port: code.port,
                    };
                    self.open(
                        code.port,
                        &format!("{PAIR_PATH_PREFIX}{}", code.pair_id_hex()),
                        Role::Pair {
                            credential,
                            sas: None,
                        },
                        handshake,
                    )
                })();
                if let Err(problem) = result {
                    self.failed(true, Failure::Fatal(problem));
                }
            }
            Input::Request(_) => {}
            Input::Tick => {
                self.tick_clock();
                let now = self.now();
                if self.retry_at.is_some_and(|deadline| now >= deadline) {
                    self.connect_session();
                    return;
                }
                let Some(mut conn) = self.connection.take() else {
                    return;
                };
                let pair = matches!(conn.role, Role::Pair { .. });
                let result = match conn.stage {
                    Stage::Opening | Stage::Handshake
                        if now.saturating_sub(conn.started) >= 5_000 =>
                    {
                        Err("corrald connection handshake timed out".into())
                    }
                    Stage::Transport => {
                        if let Some(method) = conn.core.expired(now) {
                            Err(format!("corrald {method} timed out"))
                        } else if !pair
                            && now.saturating_sub(conn.received.max(conn.pinged)) >= 15_000
                        {
                            conn.pinged = now;
                            conn.core
                                .request(wire::PING, json!({}), now)
                                .and_then(|()| conn.flush())
                        } else {
                            Ok(())
                        }
                    }
                    _ => Ok(()),
                };
                match result {
                    Ok(()) => self.connection = Some(conn),
                    Err(problem) => self.failed(pair, Failure::Retry(problem)),
                }
            }
            Input::Closed {
                generation,
                code,
                reason,
            } => {
                let Some(conn) = self.connection.take() else {
                    return;
                };
                if conn.generation != generation {
                    self.connection = Some(conn);
                    return;
                }
                if matches!(conn.role, Role::Pair { .. }) {
                    self.failed(
                        true,
                        Failure::Fatal(format!(
                            "corrald closed the pairing connection ({reason})."
                        )),
                    );
                } else {
                    match BrowserSessionClose::received(
                        &reason,
                        matches!(conn.stage, Stage::Opening | Stage::Handshake),
                    ) {
                        BrowserSessionClose::Revoked => {
                            self.retry_at = None;
                            if let Some(credential) = &self.credential {
                                self.feed.update(|feed| {
                                    feed.revoked(credential.device.clone());
                                    feed.link_lost("This browser was revoked.".into());
                                });
                            }
                        }
                        BrowserSessionClose::Failed(line) => {
                            self.failed(false, Failure::Fatal(line.into()));
                        }
                        BrowserSessionClose::Renew => self.connect_session(),
                        BrowserSessionClose::Retry => {
                            self.lost(format!("corrald closed the connection ({code}: {reason})."))
                        }
                    }
                }
            }
            Input::Opened { generation } => {
                let Some(mut conn) = self.connection.take() else {
                    return;
                };
                if conn.generation != generation {
                    self.connection = Some(conn);
                    return;
                }
                let pair = matches!(conn.role, Role::Pair { .. });
                let result = (|| {
                    let handshake = conn.handshake.as_mut().ok_or("missing Noise handshake")?;
                    let mut bytes = [0; 128];
                    let len = handshake
                        .write_message(&[], &mut bytes)
                        .map_err(|error| error.to_string())?;
                    conn.socket.send(&bytes[..len])?;
                    conn.stage = Stage::Handshake;
                    Ok::<_, String>(())
                })();
                match result {
                    Ok(()) => self.connection = Some(conn),
                    Err(problem) => self.failed(pair, Failure::Retry(problem)),
                }
            }
            Input::Message { generation, bytes } => {
                let Some(mut conn) = self.connection.take() else {
                    return;
                };
                if conn.generation != generation {
                    self.connection = Some(conn);
                    return;
                }
                conn.socket.received();
                let pair = matches!(conn.role, Role::Pair { .. });
                match self.message(&mut conn, bytes) {
                    Ok(Action::Keep) => self.connection = Some(conn),
                    Ok(Action::Stop) => {}
                    Ok(Action::Paired(credential)) => {
                        drop(conn);
                        match keystore::save(&credential).await {
                            Ok(()) => match Storage::set_item(PAIRED_KEY, "1") {
                                Ok(()) => {
                                    self.feed.update(|feed| {
                                        feed.pairing_code = None;
                                        feed.browser_pairing = BrowserPairing::Paired {
                                            device: credential.device.clone(),
                                        }
                                    });
                                    self.credential = Some(credential);
                                    self.failures = 0;
                                    self.connect_session();
                                }
                                Err(problem) => self.failed(
                                    true,
                                    Failure::Fatal(format!(
                                        "Cannot remember this browser pairing: {problem}"
                                    )),
                                ),
                            },
                            Err(problem) => self.failed(true, Failure::Fatal(problem)),
                        }
                    }
                    Err(failure) => self.failed(pair, failure),
                }
            }
        }
    }

    fn message(&mut self, conn: &mut Connection, bytes: Vec<u8>) -> Result<Action, Failure> {
        let now = self.now();
        conn.received = now;
        if matches!(conn.stage, Stage::Handshake) {
            if bytes.len() > 128 {
                return Err(Failure::Fatal(
                    "corrald sent an invalid Noise handshake".into(),
                ));
            }
            let mut handshake = conn
                .handshake
                .take()
                .ok_or_else(|| "missing Noise handshake".to_owned())?;
            let mut payload = [0; 128];
            let size = handshake
                .read_message(&bytes, &mut payload)
                .map_err(|_| Failure::Fatal(DAEMON_IDENTITY_FAILED.into()))?;
            if size != 0 {
                return Err(Failure::Fatal(
                    "corrald sent a Noise handshake payload".into(),
                ));
            }
            if let Role::Pair { sas: code, .. } = &mut conn.role {
                let sas = sas(handshake.get_handshake_hash());
                self.feed.update(|feed| {
                    feed.browser_pairing = BrowserPairing::Claimed { sas: sas.clone() }
                });
                *code = Some(sas);
            }
            conn.noise = Some(
                handshake
                    .into_transport_mode()
                    .map_err(|error| error.to_string())?,
            );
            conn.stage = Stage::Transport;
            let caps: &[&str] = if matches!(conn.role, Role::Session) {
                &[wire::BOARD_V1]
            } else {
                &[]
            };
            conn.core.hello(caps, now)?;
            conn.flush()?;
            return Ok(Action::Keep);
        }
        if !matches!(conn.stage, Stage::Transport) {
            return Err(Failure::Fatal(
                "corrald sent data before the handshake".into(),
            ));
        }
        if !(NOISE_TAG + 5..=MESSAGE_MAX + NOISE_TAG).contains(&bytes.len()) {
            return Err(Failure::Fatal(
                "corrald sent an invalid encrypted frame size".into(),
            ));
        }
        conn.plain.resize(bytes.len() - NOISE_TAG, 0);
        let noise = conn
            .noise
            .as_mut()
            .ok_or_else(|| "missing Noise transport".to_owned())?;
        let len = noise
            .read_message(&bytes, &mut conn.plain)
            .map_err(|error| {
                Failure::Fatal(format!("corrald sent an unauthenticated frame: {error}"))
            })?;
        let frame = decode_message(&conn.plain[..len])
            .map_err(|error| Failure::Fatal(error.to_string()))?;
        conn.core.receive(frame, now)?;
        if let Some(welcome) = conn.core.take_welcome() {
            match &conn.role {
                Role::Session => {
                    let credential = self
                        .credential
                        .as_ref()
                        .ok_or_else(|| "missing browser pairing".to_owned())?;
                    self.feed.update(|feed| {
                        feed.welcome_received(welcome.daemon.store, map::health(welcome.health));
                        feed.device = DeviceLink::Enrolled {
                            id: credential.device.clone(),
                            name: credential.name.clone(),
                        };
                    });
                    conn.core.open(Topic::Health, now)?;
                    conn.core.open(Topic::Board, now)?;
                }
                Role::Pair { credential, .. } => conn.core.request(
                    PAIR_CLAIM,
                    json!({"name": credential.name, "app_version": env!("CARGO_PKG_VERSION")}),
                    now,
                )?,
            }
        }
        while let Some(event) = conn.core.next_event() {
            match event {
                Event::Health(health) if matches!(conn.role, Role::Session) => self
                    .feed
                    .update(|feed| feed.health_received(map::health(health))),
                Event::Board {
                    epoch,
                    rows,
                    left,
                    replacement,
                } if matches!(conn.role, Role::Session) => {
                    self.failures = 0;
                    self.feed.update(|feed| {
                        feed.board_received(
                            epoch,
                            rows.into_iter().map(map::task).collect(),
                            &left,
                            replacement,
                        )
                    });
                }
                Event::Reply {
                    method: PAIR_CLAIM,
                    outcome,
                } => {
                    let value = outcome.map_err(|(_, message)| Failure::Fatal(message))?;
                    let claimed: PairClaimed = serde_json::from_value(value)
                        .map_err(|error| Failure::Fatal(error.to_string()))?;
                    if !matches!(&conn.role, Role::Pair { sas: Some(sas), .. } if *sas == claimed.sas)
                    {
                        return Err(Failure::Fatal(
                            "The pairing codes differ; start again.".into(),
                        ));
                    }
                }
                Event::Notice { method, params } if method == PAIR_DONE => {
                    let done: PairDone = serde_json::from_value(params)
                        .map_err(|error| Failure::Fatal(error.to_string()))?;
                    if let Role::Pair { mut credential, .. } =
                        std::mem::replace(&mut conn.role, Role::Session)
                    {
                        credential.device = done.device;
                        return Ok(Action::Paired(credential));
                    }
                    return Err(Failure::Fatal(
                        "corrald sent an unexpected pairing completion".into(),
                    ));
                }
                Event::Notice { method, params } if method == PAIR_REFUSED => {
                    let refused: PairRefused = serde_json::from_value(params)
                        .map_err(|error| Failure::Fatal(error.to_string()))?;
                    let line = match refused.reason.as_str() {
                        "expired" => "The pairing code expired; start again.".into(),
                        "cancelled" => "Pairing was cancelled on the trusted device.".into(),
                        _ => refused.reason,
                    };
                    return Err(Failure::Fatal(line));
                }
                Event::Notice { method, params }
                    if method == wire::BYE && params["reason"] == CLOSE_REVOKED =>
                {
                    if let Some(credential) = &self.credential {
                        self.feed
                            .update(|feed| feed.revoked(credential.device.clone()));
                    }
                    self.retry_at = None;
                    return Ok(Action::Stop);
                }
                Event::Closed(problem) => return Err(Failure::Retry(problem)),
                _ => {}
            }
        }
        conn.flush()?;
        Ok(Action::Keep)
    }
}

fn key(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 {
        return Err(
            "The stored browser pairing has a damaged key; forget it and pair again.".into(),
        );
    }
    let mut key = [0; 32];
    for (at, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).map_err(|_| {
            "The stored browser pairing has a damaged key; forget it and pair again.".to_owned()
        })?;
    }
    Ok(key)
}
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 15) as usize] as char);
    }
    text
}
