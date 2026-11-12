//! Blocking, bounded local RPC. Store-writing commands remain usable without a daemon.
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail};
use corral_core::Store;
#[cfg(unix)]
use corral_protocol::{
    ClientInfo, Hello, Nudge, PROTO_MAJOR, PROTO_MINOR, Proto, Stop, Welcome,
    frame::{Frame, FrameKind, read_frame, write_frame},
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
}

impl Client {
    pub fn connect(store: &Store) -> Result<Option<Self>> {
        Self::connect_at(&socket_path(store))
    }

    #[cfg(unix)]
    pub fn connect_at(socket: &Path) -> Result<Option<Self>> {
        let stream = match UnixStream::connect(socket) {
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
        let mut client = Self { stream, next_id: 0 };
        let welcome: Welcome = client.request(
            corral_protocol::HELLO,
            Hello {
                proto: Proto {
                    major: PROTO_MAJOR,
                    minor: PROTO_MINOR,
                },
                caps: Vec::new(),
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
        self.stream
            .set_read_timeout(Some(timeout))
            .context("setting daemon nudge timeout")?;
        let result = self.request(
            corral_protocol::NUDGE,
            Nudge {
                source: source.to_owned(),
                wait_ms,
            },
        );
        let reset = self
            .stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .context("restoring daemon read timeout");
        match result {
            Ok(ack) => {
                reset?;
                Ok(ack)
            }
            Err(error) => Err(error),
        }
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

    #[cfg(unix)]
    pub fn stop(&mut self, drain: bool) -> Result<()> {
        let timeout = if drain {
            Duration::from_secs(10 * 60)
        } else {
            IO_TIMEOUT
        };
        self.stream
            .set_read_timeout(Some(timeout))
            .context("setting daemon stop timeout")?;
        let result: Result<serde_json::Value> =
            self.request(corral_protocol::DAEMON_STOP, Stop { drain });
        let reset = self
            .stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .context("restoring daemon read timeout");
        match result {
            Ok(_) => {
                reset?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(not(unix))]
    pub fn stop(&mut self, _drain: bool) -> Result<()> {
        bail!("daemon sockets are unavailable on this platform")
    }

    #[cfg(unix)]
    fn request<P: Serialize, T: DeserializeOwned>(&mut self, method: &str, params: P) -> Result<T> {
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
        write_frame(
            &mut self.stream,
            &Frame {
                kind: FrameKind::Control,
                stream: 0,
                payload,
            },
        )
        .context("sending daemon request")?;
        let frame = read_frame(&mut self.stream)
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
        match response.outcome {
            ResponseOutcome::Result { result } => Ok(result),
            ResponseOutcome::Error { error } => bail!("daemon {:?}: {}", error.code, error.message),
        }
    }
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
