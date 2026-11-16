//! Browser-owned Noise sessions and shared app data; only the paired marker uses NSH Storage.
mod identity;
mod keystore;
mod lifecycle;
mod noise;
pub(crate) mod push;
mod push_runtime;
mod socket;

use super::{
    PAIRED_KEY,
    app::{self, AppState},
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
        sender.borrow().as_ref().is_some_and(|sender| {
            if matches!(&request, ClientRequest::BrowserPush { enabled: true, .. }) {
                push::begin_permission();
            }
            sender.unbounded_send(Input::Request(request)).is_ok()
        })
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
            app: AppState::new(),
            credential: None,
            connection: None,
            generation: 0,
            failures: 0,
            retry_at: None,
            forgetting: identity::ForgetState::default(),
            push_intent: None,
        };
        driver.tick_clock();
        driver.restore().await;
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

    // Each app transition registers and flushes synchronously, before the next input/event.
    // Unlike native batching, no later cancel/discard can invalidate an unregistered tuple.
    fn flush_app(&mut self, outgoing: Vec<app::Outgoing>, now: u64) -> Result<(), String> {
        for (method, params, tag) in outgoing {
            self.core.request_tagged(method, params, tag, now)?;
        }
        self.flush()
    }
}

struct Driver {
    feed: Signal<Feed>,
    sender: UnboundedSender<Input>,
    clock: web_sys::Performance,
    app: AppState,
    credential: Option<WebCredential>,
    connection: Option<Connection>,
    generation: u64,
    failures: usize,
    retry_at: Option<u64>,
    forgetting: identity::ForgetState,
    push_intent: Option<push_runtime::Intent>,
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
                );
                feed.time_zone = Some(local.time_zone().clone());
                feed.date = local.date().to_string();
            });
        }
    }

    fn open(
        &mut self,
        endpoint: &str,
        path: &str,
        role: Role,
        handshake: HandshakeState,
    ) -> Result<(), String> {
        self.connection = None;
        self.push_intent = None;
        self.retry_at = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let socket = Socket::open(endpoint, path, generation, self.sender.clone())?;
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
        if self.forgetting.pending() {
            return;
        }
        let result = (|| {
            let credential = self
                .credential
                .as_ref()
                .ok_or("This browser is not paired")?;
            let endpoint = credential
                .endpoint
                .clone()
                .unwrap_or_else(|| format!("ws://127.0.0.1:{}", credential.port));
            let private = key(&credential.private)?;
            let daemon = key(&credential.daemon)?;
            let handshake = noise::initiator_session(&private, &daemon)?;
            self.feed.update(|feed| feed.connecting(endpoint.clone()));
            self.open(&endpoint, SESSION_PATH, Role::Session, handshake)
        })();
        if let Err(problem) = result {
            self.lost(problem);
        }
    }

    fn lost(&mut self, problem: String) {
        self.connection = None;
        self.app.disconnected(self.feed);
        self.feed.update(|feed| feed.link_lost(problem));
        let delay = super::requests::reconnect_delay_ms(self.failures);
        self.failures = self.failures.saturating_add(1);
        self.push_intent = None;
        self.retry_at = self
            .credential
            .as_ref()
            .filter(|_| !self.forgetting.pending())
            .map(|_| self.now() + delay);
    }

    fn failed(&mut self, pair: bool, failure: Failure) {
        self.connection = None;
        self.push_intent = None;
        self.retry_at = None;
        match failure {
            Failure::Retry(problem) if !pair => self.lost(problem),
            Failure::Retry(problem) | Failure::Fatal(problem) => {
                self.app.disconnected(self.feed);
                self.feed.update(|feed| {
                    if pair {
                        feed.browser_pairing = BrowserPairing::Failed(problem);
                    } else {
                        feed.device = DeviceLink::Failed(problem.clone());
                        feed.link_lost(problem);
                    }
                });
            }
        }
    }

    async fn handle(&mut self, input: Input) {
        match input {
            Input::Request(ClientRequest::Forget) => self.forget().await,
            Input::Request(ClientRequest::Pair { code, name }) => {
                if self.feed.with_untracked(|feed| {
                    matches!(feed.browser_pairing, BrowserPairing::Blocked(_))
                }) {
                    return;
                }
                if self.forgetting.pending() {
                    self.forget_failed(
                        "Forgetting this browser is incomplete; retry Forget before pairing."
                            .into(),
                    );
                    return;
                }
                self.connection = None;
                self.push_intent = None;
                self.retry_at = None;
                self.app.disconnected(self.feed);
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
                        endpoint: code.endpoint.clone(),
                    };
                    self.open(
                        &code
                            .endpoint
                            .clone()
                            .unwrap_or_else(|| format!("ws://127.0.0.1:{}", code.port)),
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
            Input::Request(ClientRequest::BrowserPush {
                request_id,
                enabled,
                kinds,
            }) => {
                self.change_push(request_id, enabled, kinds).await;
            }
            Input::Request(ClientRequest::PushStatus { request_id }) => {
                self.push_status(request_id);
            }
            Input::Request(request) => {
                let outgoing = self.app.request_async(self.feed, request).await;
                if outgoing.is_empty() {
                    return;
                }
                let Some(mut conn) = self.connection.take() else {
                    return;
                };
                match conn.flush_app(outgoing, self.now()) {
                    Ok(()) => match self.process_events(&mut conn, self.now()).await {
                        Ok(Action::Keep) => self.connection = Some(conn),
                        Ok(Action::Stop) => {}
                        Ok(Action::Paired(_)) => self.failed(
                            false,
                            Failure::Fatal(
                                "Unexpected pairing completion during request admission.".into(),
                            ),
                        ),
                        Err(failure) => self.failed(false, failure),
                    },
                    Err(problem) => self.failed(false, Failure::Retry(problem)),
                }
            }
            Input::Tick => {
                self.tick_clock();
                self.app.refresh_local(self.feed).await;
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
                            && !conn.core.has_pending()
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
                let result = result.and_then(|()| {
                    if !pair && matches!(conn.stage, Stage::Transport) {
                        conn.flush_app(self.app.pair_poll(self.feed), now)
                    } else {
                        Ok(())
                    }
                });
                match result {
                    // Nothing can be sent or received before the Noise transport exists;
                    // flushing here failed every connection still opening when a tick landed.
                    Ok(()) if !matches!(conn.stage, Stage::Transport) => {
                        self.connection = Some(conn);
                    }
                    Ok(()) => match self.process_events(&mut conn, now).await {
                        Ok(Action::Keep) => self.connection = Some(conn),
                        Ok(Action::Stop) => {}
                        Ok(Action::Paired(_)) => self.failed(
                            pair,
                            Failure::Fatal(
                                "Unexpected pairing completion during timeout handling.".into(),
                            ),
                        ),
                        Err(failure) => self.failed(pair, failure),
                    },
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
                self.app.disconnected(self.feed);
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
                            self.revoke_local().await;
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
                match self.message(&mut conn, bytes).await {
                    Ok(Action::Keep) => self.connection = Some(conn),
                    Ok(Action::Stop) => {}
                    Ok(Action::Paired(credential)) => {
                        drop(conn);
                        match keystore::save(&credential).await {
                            Ok(()) => {
                                // This marker is only a hint; the credential transaction is authoritative.
                                let _ = Storage::set_item(PAIRED_KEY, "1");
                                self.feed.update(|feed| {
                                    feed.pairing_code = None;
                                    feed.browser_pairing = BrowserPairing::Paired {
                                        device: credential.device.clone(),
                                    }
                                });
                                self.credential = Some(credential);
                                self.failures = 0;
                                let device = self
                                    .credential
                                    .as_ref()
                                    .map(|credential| credential.device.clone());
                                match self.app.boot(self.feed, device.as_deref()).await {
                                    Ok(()) => self.connect_session(),
                                    Err(problem) => self.failed(false, Failure::Fatal(problem)),
                                }
                            }
                            Err(problem) => self.failed(true, Failure::Fatal(problem)),
                        }
                    }
                    Err(failure) => self.failed(pair, failure),
                }
            }
        }
    }

    async fn message(&mut self, conn: &mut Connection, bytes: Vec<u8>) -> Result<Action, Failure> {
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
                app::CAPS
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
                    self.app.welcomed(self.feed, &welcome);
                    self.feed.update(|feed| {
                        feed.welcome_received(welcome.daemon.store, map::health(welcome.health));
                        feed.device = DeviceLink::Enrolled {
                            id: credential.device.clone(),
                            name: credential.name.clone(),
                        };
                    });
                    let mut outgoing = self.app.ready(self.feed);
                    // Like a native attach: the device count stays unknown until this walk ends.
                    outgoing.push(app::devices());
                    conn.flush_app(outgoing, now)?;
                }
                Role::Pair { credential, .. } => conn.core.request(
                    PAIR_CLAIM,
                    json!({"name": credential.name, "app_version": env!("CARGO_PKG_VERSION")}),
                    now,
                )?,
            }
        }
        self.process_events(conn, now).await
    }

    async fn process_events(&mut self, conn: &mut Connection, now: u64) -> Result<Action, Failure> {
        while let Some(event) = conn.core.next_event() {
            if self.push_event(conn, &event, now).await? {
                continue;
            }
            if matches!(conn.role, Role::Session) && app::handles(&event) {
                if matches!(&event, Event::Board { .. }) {
                    self.failures = 0;
                }
                let outgoing = self.app.event_async(self.feed, event).await?;
                conn.flush_app(outgoing, now)?;
                continue;
            }
            match event {
                Event::Reply {
                    method: PAIR_CLAIM,
                    outcome,
                    ..
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
                    self.revoke_local().await;
                    return Ok(Action::Stop);
                }
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
