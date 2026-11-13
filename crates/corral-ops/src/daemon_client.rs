//! Blocking, bounded local RPC. Store-writing commands remain usable without a daemon.
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail};
use corral_core::Store;
use corral_protocol::board::BoardRow;
use corral_protocol::read::ReadKind;
#[cfg(unix)]
use corral_protocol::read::{ReadReply, ReadRequest};
#[cfg(unix)]
use corral_protocol::subscription::{SubAck, SubClose, SubDelta, SubOpen, SubReset, Topic};
#[cfg(unix)]
use corral_protocol::{
    ClientInfo, Hello, Nudge, PROTO_MAJOR, PROTO_MINOR, Proto, Stop, Welcome,
    frame::{Frame, FrameKind, read_frame_until, write_frame_until},
    rpc::{JSONRPC, Request, Response, ResponseOutcome},
};
use corral_protocol::{Health, NudgeAck};
#[cfg(unix)]
use serde::{Serialize, de::DeserializeOwned};
#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(all(test, unix))]
#[path = "daemon_client_tests.rs"]
mod tests;

#[cfg(unix)]
const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub fn runtime_dir(store: &Store) -> PathBuf {
    runtime_dir_of(store.dir())
}

/// `store_dir` is the `.corral` directory, not its workspace parent.
/// For discovery without a store open, use `Store::find_root(start)?.join(".corral")`.
pub fn runtime_dir_of(store_dir: &Path) -> PathBuf {
    crate::agent::launch::runtime_root().join(corral_core::agent::paths::store_id_of(store_dir))
}

pub fn socket_path(store: &Store) -> PathBuf {
    socket_path_of(store.dir())
}

pub fn socket_path_of(store_dir: &Path) -> PathBuf {
    runtime_dir_of(store_dir).join("corrald.sock")
}

pub fn heartbeat_path(store: &Store) -> PathBuf {
    runtime_dir(store).join("heartbeat")
}

pub struct Client {
    #[cfg(unix)]
    stream: UnixStream,
    #[cfg(unix)]
    next_id: u64,
    #[cfg(unix)]
    read_capable: bool,
}

impl Client {
    pub fn connect(store: &Store) -> Result<Option<Self>> {
        Self::connect_at(&socket_path(store))
    }

    #[cfg(unix)]
    pub fn connect_at(socket: &Path) -> Result<Option<Self>> {
        let stream = connect_socket(socket, Instant::now() + IO_TIMEOUT);
        let stream = match stream {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("connecting to {}", socket.display()));
            }
        };
        check_peer(&stream)?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .context("setting daemon read timeout")?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .context("setting daemon write timeout")?;
        let mut client = Self {
            stream,
            next_id: 0,
            read_capable: false,
        };
        let welcome: Welcome = client.request(
            corral_protocol::HELLO,
            Hello {
                proto: Proto {
                    major: PROTO_MAJOR,
                    minor: PROTO_MINOR,
                },
                caps: vec![
                    corral_protocol::BOARD_V1.into(),
                    corral_protocol::read::READ_V1.into(),
                ],
                client: ClientInfo {
                    kind: "cli".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            },
        )?;
        if welcome.proto.major != PROTO_MAJOR {
            bail!(
                "unsupported daemon protocol major {} (client uses {PROTO_MAJOR})",
                welcome.proto.major
            );
        }
        client.read_capable = welcome
            .caps
            .iter()
            .any(|cap| cap == corral_protocol::read::READ_V1);
        Ok(Some(client))
    }

    #[cfg(not(unix))]
    pub fn connect_at(_socket: &Path) -> Result<Option<Self>> {
        Ok(None)
    }

    #[cfg(unix)]
    pub fn nudge(&mut self, source: &str, wait: Option<Duration>) -> Result<NudgeAck> {
        let wait_ms = wait
            .map(|wait| {
                u32::try_from(wait.as_millis())
                    .context("daemon nudge wait exceeds u32 milliseconds")
            })
            .transpose()?;
        let timeout = IO_TIMEOUT + Duration::from_millis(u64::from(wait_ms.unwrap_or(0)));
        self.request_with_timeout(
            corral_protocol::NUDGE,
            Nudge {
                source: source.to_owned(),
                wait_ms,
            },
            timeout,
        )
    }

    #[cfg(not(unix))]
    pub fn nudge(&mut self, _source: &str, _wait: Option<Duration>) -> Result<NudgeAck> {
        bail!("daemon sockets are unavailable on this platform")
    }

    #[cfg(unix)]
    pub fn health(&mut self) -> Result<Health> {
        self.request(corral_protocol::HEALTH_GET, corral_protocol::HealthGet {})
    }

    #[cfg(not(unix))]
    pub fn health(&mut self) -> Result<Health> {
        bail!("daemon sockets are unavailable on this platform")
    }

    /// Read one served JSON document, bounded by 10 s for the reply and 5 s for data.
    #[cfg(unix)]
    pub fn read(&mut self, kind: ReadKind) -> Result<Option<Vec<u8>>> {
        if !self.read_capable {
            return Ok(None);
        }
        let (exe, build) = crate::pm_read::client_identity()?;
        let outcome: ResponseOutcome<ReadReply> = self.request_outcome(
            corral_protocol::read::READ,
            ReadRequest { kind, exe, build },
            Duration::from_secs(10),
        )?;
        let reply = match outcome {
            ResponseOutcome::Result { result } => result,
            ResponseOutcome::Error { error } => {
                use corral_protocol::rpc::ErrorCode;
                return match error.code {
                    ErrorCode::Refused
                    | ErrorCode::Busy
                    | ErrorCode::Unavailable
                    | ErrorCode::MethodNotFound
                    | ErrorCode::Internal => Ok(None),
                    _ => bail!("daemon {:?}: {}", error.code, error.message),
                };
            }
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut document = Vec::new();
        for _ in 0..reply.frames {
            let frame = read_frame_until(&mut self.stream, deadline)
                .context("reading served document")?
                .context("daemon closed before completing served document")?;
            if frame.kind != FrameKind::Data || frame.stream != reply.stream {
                bail!("daemon returned an unexpected served document frame");
            }
            let bytes = (document.len() as u64)
                .checked_add(frame.payload.len() as u64)
                .context("served document length overflow")?;
            if bytes > reply.bytes {
                bail!("served document exceeds announced byte count");
            }
            document.extend_from_slice(&frame.payload);
        }
        if document.len() as u64 != reply.bytes {
            bail!("served document does not match announced byte count");
        }
        Ok(Some(document))
    }

    #[cfg(not(unix))]
    pub fn read(&mut self, _kind: ReadKind) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    #[cfg(unix)]
    pub fn stop(&mut self, drain: bool) -> Result<()> {
        let timeout = if drain {
            Duration::from_secs(10 * 60)
        } else {
            IO_TIMEOUT
        };
        let _: serde_json::Value =
            self.request_with_timeout(corral_protocol::DAEMON_STOP, Stop { drain }, timeout)?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn stop(&mut self, _drain: bool) -> Result<()> {
        bail!("daemon sockets are unavailable on this platform")
    }

    /// Read the daemon's current board, acknowledging each bounded page.
    #[cfg(unix)]
    pub fn board(&mut self) -> Result<Vec<BoardRow>> {
        let mut rows = std::collections::BTreeMap::new();
        let mut resets = 0;
        let mut epoch = None;
        self.open_board()?;
        loop {
            let frame = read_frame_until(&mut self.stream, Instant::now() + IO_TIMEOUT)
                .context("reading board subscription")?
                .context("daemon closed board subscription")?;
            if frame.kind != FrameKind::Control || frame.stream != 0 {
                bail!("daemon returned a non-control board frame");
            }
            let notification: corral_protocol::rpc::Notification =
                serde_json::from_slice(&frame.payload).context("decoding board notification")?;
            if notification.jsonrpc != JSONRPC {
                bail!("invalid board notification version");
            }
            match notification.method.as_str() {
                corral_protocol::SUB_RESET => {
                    let reset: SubReset = serde_json::from_value(notification.params)
                        .context("decoding board reset")?;
                    if reset.topic != Topic::Board || resets != 0 {
                        bail!("board projection reset repeatedly or returned the wrong topic");
                    }
                    resets += 1;
                    rows.clear();
                    epoch = None;
                    self.open_board()?;
                }
                corral_protocol::SUB_DELTA => {
                    let delta: SubDelta = serde_json::from_value(notification.params)
                        .context("decoding board delta")?;
                    let SubDelta::Board { delta } = delta else {
                        bail!("daemon returned the wrong subscription topic");
                    };
                    if epoch.is_some_and(|epoch| epoch != delta.epoch) {
                        bail!("board epoch changed without sub.reset");
                    }
                    epoch = Some(delta.epoch);
                    for row in delta.rows {
                        rows.insert(row.id.clone(), row);
                    }
                    for id in delta.left {
                        rows.remove(&id);
                    }
                    if !delta.more {
                        let _: serde_json::Value = self.request(
                            corral_protocol::SUB_CLOSE,
                            SubClose {
                                topic: Topic::Board,
                            },
                        )?;
                        return Ok(rows.into_values().collect());
                    }
                    let _: serde_json::Value = self.request(
                        corral_protocol::SUB_ACK,
                        SubAck {
                            topic: Topic::Board,
                            cursor: delta.cursor,
                        },
                    )?;
                }
                _ => bail!("daemon closed or failed the board subscription"),
            }
        }
    }

    #[cfg(unix)]
    fn open_board(&mut self) -> Result<()> {
        let _: serde_json::Value = self.request(
            corral_protocol::SUB_OPEN,
            SubOpen {
                topic: Topic::Board,
                since: None,
            },
        )?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn board(&mut self) -> Result<Vec<BoardRow>> {
        bail!("daemon sockets are unavailable on this platform")
    }

    #[cfg(unix)]
    fn request<P: Serialize, T: DeserializeOwned>(&mut self, method: &str, params: P) -> Result<T> {
        self.request_with_timeout(method, params, IO_TIMEOUT)
    }

    #[cfg(unix)]
    fn request_with_timeout<P: Serialize, T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<T> {
        match self.request_outcome(method, params, timeout)? {
            ResponseOutcome::Result { result } => Ok(result),
            ResponseOutcome::Error { error } => bail!("daemon {:?}: {}", error.code, error.message),
        }
    }

    #[cfg(unix)]
    fn request_outcome<P: Serialize, T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<ResponseOutcome<T>> {
        let deadline = Instant::now() + timeout;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("daemon request ids exhausted")?;
        let id = serde_json::Value::from(self.next_id);
        let request = Request {
            jsonrpc: JSONRPC.into(),
            id: id.clone(),
            method: method.into(),
            params,
        };
        let payload = serde_json::to_vec(&request).context("encoding daemon request")?;
        write_frame_until(
            &mut self.stream,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload,
            },
            Instant::now() + IO_TIMEOUT,
        )
        .context("sending daemon request")?;
        let frame = read_frame_until(&mut self.stream, deadline)
            .context("reading daemon response")?
            .context("daemon closed before responding")?;
        if frame.kind != FrameKind::Control || frame.stream != 0 {
            bail!("daemon returned a non-control RPC response");
        }
        let response: Response<T> =
            serde_json::from_slice(&frame.payload).context("decoding daemon response")?;
        if response.jsonrpc != JSONRPC || response.id != id {
            bail!("daemon returned a mismatched RPC response");
        }
        Ok(response.outcome)
    }
}

#[cfg(unix)]
fn connect_socket(socket: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let flags = SocketFlags::NONBLOCK | SocketFlags::CLOEXEC;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let flags = SocketFlags::empty();
    let fd = rustix::net::socket_with(AddressFamily::UNIX, SocketType::STREAM, flags, None)?;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let descriptor_flags = rustix::io::fcntl_getfd(&fd)?;
        rustix::io::fcntl_setfd(&fd, descriptor_flags | rustix::io::FdFlags::CLOEXEC)?;
        let status_flags = rustix::fs::fcntl_getfl(&fd)?;
        rustix::fs::fcntl_setfl(&fd, status_flags | rustix::fs::OFlags::NONBLOCK)?;
    }
    let address = SocketAddrUnix::new(socket)?;
    match rustix::net::connect(&fd, &address) {
        Ok(()) => {}
        Err(rustix::io::Errno::INPROGRESS) => loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "daemon connect deadline expired",
                    )
                })?;
            let timeout = rustix::event::Timespec::try_from(remaining)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
            match rustix::event::poll(
                &mut [rustix::event::PollFd::new(
                    &fd,
                    rustix::event::PollFlags::OUT,
                )],
                Some(&timeout),
            ) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "daemon connect deadline expired",
                    ));
                }
                Ok(_) => {
                    rustix::net::sockopt::socket_error(&fd)??;
                    break;
                }
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
        },
        Err(error) => return Err(error.into()),
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn check_peer(stream: &UnixStream) -> Result<()> {
    let peer =
        rustix::net::sockopt::socket_peercred(stream).context("reading daemon peer credentials")?;
    if peer.uid != rustix::process::getuid() {
        bail!("daemon peer uid does not match this user");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_peer(stream: &UnixStream) -> Result<()> {
    use std::os::fd::AsRawFd;

    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: the stream owns a live socket descriptor and both output pointers are valid.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(std::io::Error::last_os_error()).context("reading daemon peer credentials");
    }
    if uid != rustix::process::getuid().as_raw() {
        bail!("daemon peer uid does not match this user");
    }
    Ok(())
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_os = "macos"))
))]
fn check_peer(_stream: &UnixStream) -> Result<()> {
    bail!("daemon peer credentials are unavailable on this platform")
}

pub fn nudge(store: &Store, source: &str) -> bool {
    nudge_at(store.dir(), source)
}

/// Best effort: an absent or failed daemon never prevents a direct store write.
pub fn nudge_at(store_dir: &Path, source: &str) -> bool {
    Client::connect_at(&socket_path_of(store_dir))
        .ok()
        .flatten()
        .is_some_and(|mut client| client.nudge(source, None).is_ok())
}

pub fn health(store: &Store) -> Health {
    Client::connect(store)
        .ok()
        .flatten()
        .and_then(|mut client| client.health().ok())
        .unwrap_or_else(Health::down)
}

/// Best effort: any daemon or protocol failure falls back to an in-process read.
pub fn served(store: &Store, kind: ReadKind) -> Option<Vec<u8>> {
    Client::connect(store)
        .ok()
        .flatten()?
        .read(kind)
        .ok()
        .flatten()
}
