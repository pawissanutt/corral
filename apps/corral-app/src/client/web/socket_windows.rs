//! Win32 carrier for the shared Noise driver. Network work never runs on the NSH owner.
use std::{
    io,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use corral_protocol::device::{CLOSE_PROTOCOL, valid_endpoint};
use futures_channel::mpsc::UnboundedSender;
use futures_util::{SinkExt, StreamExt};
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::{
    net::TcpStream,
    runtime::Runtime,
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    Connector, client_async_tls_with_config,
    tungstenite::{
        Bytes, Message, client::IntoClientRequest, handshake::client::Request,
        protocol::WebSocketConfig,
    },
};

use super::{HANDSHAKE_DEADLINE_MS, Input, MAX_MESSAGE_BYTES, MAX_QUEUED_MESSAGES};

const TRANSFER_DEADLINE: Duration = Duration::from_secs(5);
const ABNORMAL_CLOSE: u16 = 1006;
const POLICY_CLOSE: u16 = 1008;

pub(super) type MessageBytes = Bytes;

pub(super) struct Socket {
    task: JoinHandle<()>,
    outgoing: mpsc::Sender<MessageBytes>,
    queued: Arc<AtomicUsize>,
}

/// The endpoint is only an origin: protocol paths are appended by the driver.
/// Plaintext is restricted to the explicit Windows-to-WSL loopback address.
pub(super) fn normalize_endpoint(endpoint: &str) -> Result<String, String> {
    const INVALID: &str = "Enter a daemon address such as ws://127.0.0.1:7841 or wss://host.";
    if !valid_endpoint(endpoint) {
        return Err(INVALID.into());
    }
    if let Some(port) = endpoint.strip_prefix("ws://127.0.0.1:") {
        let port = port.parse::<u16>().map_err(|_| INVALID)?;
        return Ok(format!("ws://127.0.0.1:{port}"));
    }
    if !endpoint.starts_with("wss://") {
        return Err(INVALID.into());
    }
    let url = url::Url::parse(endpoint).map_err(|_| INVALID)?;
    Ok(url.origin().ascii_serialization())
}

impl Socket {
    pub(super) fn open(
        endpoint: &str,
        path: &str,
        generation: u64,
        tx: UnboundedSender<Input>,
    ) -> Result<Self, String> {
        let endpoint = normalize_endpoint(endpoint)?;
        if !path.starts_with('/') {
            return Err("the daemon socket protocol path is invalid".into());
        }
        let (scheme, authority) = endpoint.split_once("://").ok_or("invalid daemon address")?;
        let origin_scheme = if scheme == "wss" { "https" } else { "http" };
        let origin = format!("{origin_scheme}://{authority}");
        let mut request = format!("{endpoint}{path}")
            .into_client_request()
            .map_err(|error| format!("invalid daemon socket request: {error}"))?;
        request.headers_mut().insert(
            "Origin",
            origin.parse().map_err(|_| "invalid daemon socket origin")?,
        );
        let (outgoing, receiver) = mpsc::channel(MAX_QUEUED_MESSAGES);
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_in_worker = Arc::clone(&queued);
        let task = runtime()?.spawn(async move {
            if let Err((code, reason)) =
                pump(request, receiver, &queued_in_worker, generation, &tx).await
            {
                let _ = tx.unbounded_send(Input::Closed {
                    generation,
                    code,
                    reason,
                });
            }
        });
        Ok(Self {
            task,
            outgoing,
            queued,
        })
    }

    pub(super) fn send(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("the daemon socket message is too large".into());
        }
        let permit = self.outgoing.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(()) => "the daemon socket send queue is full",
            mpsc::error::TrySendError::Closed(()) => "the daemon socket is not open",
        })?;
        // The driver reuses its Noise buffer on the owner thread; the worker owns each queued frame.
        permit.send(Bytes::copy_from_slice(bytes));
        Ok(())
    }

    pub(super) fn received(&self) {
        self.queued.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) fn runtime() -> Result<&'static Runtime, String> {
    static RUNTIME: LazyLock<Result<Runtime, String>> = LazyLock::new(|| {
        // All opens, including cancelled DNS lookups, share this bounded pool.
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .thread_name("corral-websocket")
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the daemon socket worker: {error}"))
    });
    RUNTIME.as_ref().map_err(Clone::clone)
}

async fn connect_tcp(host: &str, port: u16) -> io::Result<TcpStream> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return TcpStream::connect(SocketAddr::new(ip, port)).await;
    }

    // An OS lookup that was already running may outlive its cancelled opener.
    // Keep the slot in the lookup so reconnects cannot queue more of them.
    static DNS_SLOT: Semaphore = Semaphore::const_new(1);
    let permit = DNS_SLOT.acquire().await.map_err(io::Error::other)?;
    let host = host.to_owned();
    let addresses = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        (host.as_str(), port).to_socket_addrs()
    })
    .await
    .map_err(io::Error::other)??;
    TcpStream::connect(addresses.as_slice()).await
}

fn windows_tls() -> Result<Connector, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|builder| builder.with_platform_verifier())
        .map_err(|error| format!("configuring Windows TLS verification: {error}"))?
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

type Closed = (u16, String);

fn abnormal(reason: impl Into<String>) -> Closed {
    (ABNORMAL_CLOSE, reason.into())
}

async fn pump(
    request: Request,
    mut outgoing: mpsc::Receiver<MessageBytes>,
    queued: &AtomicUsize,
    generation: u64,
    tx: &UnboundedSender<Input>,
) -> Result<(), Closed> {
    let config = WebSocketConfig::default()
        .read_buffer_size(8 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_MESSAGE_BYTES + 1024)
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
        .max_message_size(Some(MAX_MESSAGE_BYTES));
    let connect = async {
        let uri = request.uri();
        let host = uri.host().ok_or("daemon socket request has no host")?;
        let host = host.trim_matches(['[', ']']);
        // Always give WSS an explicit verifier. Another crate may enable the
        // WebSocket library's bundled certificate roots for its own connections.
        let (default_port, connector) = if uri.scheme_str() == Some("wss") {
            (443, windows_tls()?)
        } else {
            (80, Connector::Plain)
        };
        let port = uri.port_u16().unwrap_or(default_port);
        let tcp = connect_tcp(host, port)
            .await
            .map_err(|error| format!("connecting to daemon: {error}"))?;
        tcp.set_nodelay(true)
            .map_err(|error| format!("configuring daemon socket: {error}"))?;
        client_async_tls_with_config(request, tcp, Some(config), Some(connector))
            .await
            .map(|(socket, _)| socket)
            .map_err(|error| format!("opening daemon WebSocket: {error}"))
    };
    let mut socket = timeout(Duration::from_millis(HANDSHAKE_DEADLINE_MS), connect)
        .await
        .map_err(|_| abnormal("daemon WebSocket upgrade timed out"))?
        .map_err(abnormal)?;
    if tx.unbounded_send(Input::Opened { generation }).is_err() {
        return Ok(());
    }

    loop {
        tokio::select! {
            outgoing = outgoing.recv() => {
                let Some(bytes) = outgoing else { return Ok(()) };
                timeout(TRANSFER_DEADLINE, socket.send(Message::Binary(bytes)))
                    .await
                    .map_err(|_| abnormal("writing daemon socket timed out"))?
                    .map_err(|error| abnormal(format!("writing daemon socket: {error}")))?;
            }
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    if queued.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                        (count < MAX_QUEUED_MESSAGES).then_some(count + 1)
                    }).is_err() {
                        return Err((POLICY_CLOSE, "client_backlog".into()));
                    }
                    if tx.unbounded_send(Input::Message { generation, bytes }).is_err() {
                        return Ok(());
                    }
                }
                Some(Ok(Message::Close(frame))) => {
                    let closed = frame.map_or_else(
                        || (1005, String::new()),
                        |frame| (u16::from(frame.code), frame.reason.to_string()),
                    );
                    // Tungstenite queued the acknowledgement on read.
                    let _ = timeout(TRANSFER_DEADLINE, socket.flush()).await;
                    return Err(closed);
                }
                Some(Ok(Message::Ping(_))) => {
                    timeout(TRANSFER_DEADLINE, socket.flush())
                        .await
                        .map_err(|_| abnormal("writing daemon socket pong timed out"))?
                        .map_err(|error| abnormal(format!("writing daemon socket pong: {error}")))?;
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(_)) => return Err((POLICY_CLOSE, CLOSE_PROTOCOL.into())),
                Some(Err(error)) => return Err(abnormal(format!("reading daemon socket: {error}"))),
                None => return Err(abnormal("daemon socket disconnected")),
            }
        }
    }
}
